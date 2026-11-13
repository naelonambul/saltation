//! Step 4 proof: listener separation and the response envelope.
//! - `/internal` exists on the generator listener and is 404 on the public one
//! - `/metrics` exists only on the metrics listener
//! - every response echoes the request ID; envelopes carry it too
//!
//! Simulated-context trust per listener is unit-tested in `shared::context`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bank::app::{self, AppState};
use bank::shared::clock::Clock;
use bank::shared::db::{Db, DbConfig};
use bank::shared::rollover::Registry;
use bank::shared::telemetry;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn state() -> AppState {
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");
    db.migrate().await.expect("migrate");
    let clock = Clock::init(db.clone(), Registry::default())
        .await
        .expect("clock");
    AppState {
        db,
        clock,
        limits: Default::default(),
    }
}

async fn get(
    router: &Router,
    path: &str,
    request_id: Option<&str>,
) -> (StatusCode, Option<String>, serde_json::Value) {
    let mut req = Request::get(path);
    if let Some(id) = request_id {
        req = req.header("x-request-id", id);
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let header = res
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, header, json)
}

#[tokio::test]
async fn listeners_expose_only_their_routes() {
    let state = state().await;
    let expected_mode =
        serde_json::to_value(state.clock.state().await.expect("clock").mode).expect("mode");
    let public = app::public_router(state.clone());
    let generator = app::generator_router(state);
    let metrics = app::metrics_router(telemetry::metrics_handle());
    let id = ulid::Ulid::new().to_string();

    let (status, header, body) = get(&generator, "/internal/v1/clock", Some(&id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header.as_deref(), Some(id.as_str()), "X-Request-Id echoed");
    assert_eq!(body["rsp_code"], "A0000");
    assert_eq!(body["request_id"], id.as_str());
    assert_eq!(body["mode"], expected_mode);
    assert_eq!(
        body["api_tran_dtm"].as_str().map(str::len),
        Some(17),
        "api_tran_dtm is N(17)"
    );

    let (status, header, _) = get(&public, "/internal/v1/clock", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "internal routes are absent on the public listener"
    );
    assert!(
        header.is_some(),
        "a generated request ID is echoed even on 404"
    );

    let (status, _, _) = get(&public, "/internal/v1/sms/01000000000", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for path in ["/oauth/2.0/token", "/v2.0/transfer/deposit/fin_num"] {
        let res = public
            .clone()
            .oneshot(Request::post(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(
            res.status(),
            StatusCode::NOT_FOUND,
            "{path} is generator only"
        );
    }

    for router in [&public, &generator] {
        let (status, _, _) = get(router, "/metrics", None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "metrics are only on the metrics listener"
        );
        let (status, _, _) = get(router, "/healthz", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    let res = metrics
        .oneshot(
            Request::get("/metrics")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::OK);
    let text = String::from_utf8(
        res.into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec(),
    )
    .expect("utf8");
    assert!(
        text.contains("bank_http_request_duration_seconds"),
        "request latency histogram exported"
    );
    assert!(
        text.contains("bank_db_statement_duration_seconds"),
        "statement latency histogram exported"
    );
}
