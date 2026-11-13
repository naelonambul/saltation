//! Password reset lifecycle, revocation, lock recovery and event secrecy.
mod common;
use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{Api, api, assert_contract, call, envelope_errors, new_id, unique_digits};
use serde_json::{Value, json};
const EVENTS: SqlStmt = sql_stmt!("test.reset_events", "sql/reset_events.sql");
const SESSION: SqlStmt = sql_stmt!("test.reset_session", "sql/reset_session.sql");
const METADATA: SqlStmt = sql_stmt!("test.reset_metadata", "sql/reset_metadata.sql");
const OLD: &str = "old-reset-password";
const NEW: &str = "new-reset-password";
async fn post(api: &Api, action: &str, body: Value) -> (StatusCode, Value) {
    call(
        &api.public,
        "POST",
        &format!("/v2.0/ext/auth/{action}"),
        &new_id(),
        None,
        Some(body),
    )
    .await
}
async fn login(api: &Api, phone: &str, password: &str) -> (StatusCode, Value) {
    post(api, "login", json!({"phone": phone, "password": password})).await
}
async fn request(api: &Api, phone: &str) -> Value {
    let (status, body) = post(api, "password_reset/request", json!({"phone": phone})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("Envelope", &body);
    body
}
async fn code(api: &Api, phone: &str) -> String {
    let (status, body) = call(
        &api.generator,
        "GET",
        &format!("/internal/v1/sms/{phone}"),
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("SmsResponse", &body);
    // The same phone, which has a message, is not readable on the public listener.
    let (status, _) = call(
        &api.public,
        "GET",
        &format!("/internal/v1/sms/{phone}"),
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    chrono::DateTime::parse_from_rfc3339(body["occurred_at"].as_str().unwrap()).unwrap();
    let code = body["code"].as_str().unwrap();
    assert_eq!(body["phone"], phone);
    assert_eq!(
        body["message"],
        format!("[살테이션] 비밀번호 재설정 인증번호 {code}")
    );
    code.to_owned()
}
async fn confirm(api: &Api, phone: &str, code: &str, expected: StatusCode) {
    let (status, body) = post(
        api,
        "password_reset/confirm",
        json!({"phone": phone, "code": code, "new_password": NEW}),
    )
    .await;
    assert_eq!(status, expected, "{body}");
    assert_contract(
        if expected == StatusCode::OK {
            "Envelope"
        } else {
            "Error"
        },
        &body,
    );
    assert_eq!(
        body["rsp_code"],
        if expected == StatusCode::OK {
            "A0000"
        } else {
            "O0002"
        }
    );
}
fn wrong(code: &str) -> &'static str {
    if code == "000000" { "000001" } else { "000000" }
}
#[tokio::test]
async fn password_reset_lifecycle() {
    let api = api().await;
    let n = unique_digits(10);
    let phone = format!("010{n}");
    let unknown = format!("019{n}");
    let no = format!("R{n}");
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None,
        Some(json!({"customers": [{"customer_no": no, "name": "재설정", "phone": phone, "customer_type": "INDIVIDUAL", "password": OLD}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = login(&api, &phone, OLD).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["access_token"].as_str().unwrap().to_owned();
    let mut known = request(&api, &phone).await;
    let mut absent = request(&api, &unknown).await;
    for field in ["api_tran_id", "api_tran_dtm", "request_id"] {
        known.as_object_mut().unwrap().remove(field);
        absent.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(known, absent);
    let (status, body) = call(
        &api.generator,
        "GET",
        &format!("/internal/v1/sms/{unknown}"),
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_contract("Error", &body);
    confirm(&api, &unknown, "123456", StatusCode::UNAUTHORIZED).await;
    let first = code(&api, &phone).await;
    let mut secrets = vec![first.clone(), OLD.to_owned(), NEW.to_owned()];
    confirm(&api, &phone, wrong(&first), StatusCode::UNAUTHORIZED).await;
    confirm(&api, &phone, &first, StatusCode::OK).await;
    confirm(&api, &phone, &first, StatusCode::UNAUTHORIZED).await;
    assert_eq!(login(&api, &phone, OLD).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&api, &phone, NEW).await.0, StatusCode::OK);
    let (status, _) = call(
        &api.public,
        "POST",
        "/oauth/2.0/revoke",
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let hash = bank::shared::auth::token_hash(&token);
    let (reason, ended, revoked): (String, i64, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&SESSION, &[&hash]))
        .await
        .unwrap();
    assert_eq!(reason, "password_reset");
    assert_eq!((ended, revoked), (1, 1));

    request(&api, &phone).await;
    let previous = code(&api, &phone).await;
    request(&api, &phone).await;
    let current = code(&api, &phone).await;
    secrets.extend([previous.clone(), current.clone()]);
    confirm(&api, &phone, &previous, StatusCode::UNAUTHORIZED).await;
    confirm(&api, &phone, &current, StatusCode::OK).await;
    request(&api, &phone).await;
    let exhausted = code(&api, &phone).await;
    secrets.push(exhausted.clone());
    for _ in 0..5 {
        confirm(&api, &phone, wrong(&exhausted), StatusCode::UNAUTHORIZED).await;
    }
    confirm(&api, &phone, &exhausted, StatusCode::UNAUTHORIZED).await;
    for _ in 0..5 {
        assert_eq!(login(&api, &phone, OLD).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&api, &phone, NEW).await.0, StatusCode::UNAUTHORIZED);
    request(&api, &phone).await;
    let recovery = code(&api, &phone).await;
    secrets.push(recovery.clone());
    confirm(&api, &phone, &recovery, StatusCode::OK).await;
    assert_eq!(login(&api, &phone, NEW).await.0, StatusCode::OK);

    for phone in ["".to_owned(), "1".repeat(21)] {
        let (status, body) = post(&api, "password_reset/request", json!({"phone": phone})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["rsp_code"], "A0002");
        assert_contract("Error", &body);
    }
    for (code, password) in [
        ("12345", NEW.to_owned()),
        ("12345x", NEW.to_owned()),
        ("１２３４５６", NEW.to_owned()),
        ("123456", "x".repeat(7)),
        ("123456", "x".repeat(129)),
    ] {
        let (status, body) = post(
            &api,
            "password_reset/confirm",
            json!({"phone": phone, "code": code, "new_password": password}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["rsp_code"], "A0002");
        assert_contract("Error", &body);
    }
    let key = no.clone();
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENTS, &[&key]))
        .await
        .unwrap();
    let parsed: Vec<Value> = events
        .iter()
        .map(|raw| serde_json::from_str(raw).unwrap())
        .collect();
    for expected in [
        "password_reset.requested",
        "password_reset.failed",
        "password_reset.completed",
    ] {
        assert!(parsed.iter().any(|event| event["event_type"] == expected));
    }
    for (event, raw) in parsed.iter().zip(&events) {
        assert!(envelope_errors(event).is_empty(), "{event}");
        for secret in &secrets {
            assert!(!raw.contains(secret), "event contains secret");
        }
    }
    let records: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&METADATA, &[&no, &no]))
        .await
        .unwrap();
    assert!(!records.is_empty());
    for record in records {
        for secret in &secrets {
            assert!(!record.contains(secret), "non-SMS record contains secret");
        }
    }
}
