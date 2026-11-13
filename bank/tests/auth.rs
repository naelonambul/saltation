//! Step 5 proof: seeding, login, wrong password, unknown phone, lockout at five
//! failures, logout, and one outbox event per attempt in the same transaction.

mod common;

use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{api, call, new_id, unique_digits};
use serde_json::json;

const EVENTS: SqlStmt = sql_stmt!(
    "test.count_events_for_request",
    "sql/count_events_for_request.sql"
);
const ATTEMPT: SqlStmt = sql_stmt!(
    "test.login_attempt_for_request",
    "sql/login_attempt_for_request.sql"
);

async fn events(api: &common::Api, request_id: &str) -> String {
    let r = request_id.to_owned();
    api.state
        .db
        .run(move |s| s.query_row::<Option<String>>(&EVENTS, &[&r]))
        .await
        .expect("events")
        .unwrap_or_default()
}

async fn attempt(api: &common::Api, request_id: &str) -> (String, String) {
    let r = request_id.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&ATTEMPT, &[&r]))
        .await
        .expect("attempt")
}

#[tokio::test]
async fn login_lockout_logout_and_events() {
    let api = api().await;
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");
    let account_no = format!("9980{n}");

    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None, Some(json!({
        "customers": [{ "customer_no": customer_no, "name": "김테스트", "phone": phone, "address": "서울",
                        "customer_type": "INDIVIDUAL", "password": "right-pass-1" }]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/accounts",
        &new_id(),
        None,
        Some(json!({
            "accounts": [{ "customer_no": customer_no, "account_no": account_no }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["accounts"][0]["fintech_use_num"]
            .as_str()
            .map(str::len),
        Some(24)
    );

    // Seeding is internal only.
    let (status, _) = call(
        &api.public,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Success from a new device: login.succeeded and device.new.
    let r = new_id();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1", "device_id": "dev-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().expect("token").to_owned();
    assert_eq!(body["scope"], "login inquiry transfer");
    assert_eq!(body["user_seq_no"].as_str().map(str::len), Some(10));
    assert_eq!(
        events(&api, &r).await,
        "login.succeeded,device.new,request.completed"
    );
    assert_eq!(attempt(&api, &r).await, ("SUCCESS".into(), "Y".into()));

    // Same device again: no device.new.
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1", "device_id": "dev-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&api, &r).await, "login.succeeded,request.completed");

    // Logout revokes the token.
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/oauth/2.0/revoke",
        &r,
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&api, &r).await, "logout,request.completed");
    let (status, body) = call(
        &api.generator,
        "POST",
        "/oauth/2.0/revoke",
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked token is rejected: {body}"
    );

    // Unknown phone: recorded without a customer.
    let r = new_id();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": format!("019{n}"), "password": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["rsp_code"], "O0001");
    assert_eq!(
        attempt(&api, &r).await,
        ("UNKNOWN_PHONE".into(), "N".into())
    );
    assert_eq!(events(&api, &r).await, "login.failed,request.completed");

    // Five wrong passwords lock the credential; the correct password is then refused.
    for i in 1..=5 {
        let r = new_id();
        let (status, body) = call(
            &api.generator,
            "POST",
            "/v2.0/ext/auth/login",
            &r,
            None,
            Some(json!({ "phone": phone, "password": "wrong" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(attempt(&api, &r).await.0, "WRONG_PASSWORD");
        assert_eq!(events(&api, &r).await, "login.failed,request.completed");
        if i == 5 {
            assert!(
                body["rsp_message"].as_str().unwrap_or("").contains("잠겼"),
                "fifth failure reports the lock"
            );
        }
    }
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(attempt(&api, &r).await.0, "LOCKED");
    assert_eq!(events(&api, &r).await, "login.failed,request.completed");
}
