use super::{Api, call, form_call, new_id};
use axum::http::StatusCode;
use serde_json::{Value, json};

pub const ENDPOINT: &str = "/card/v1/authorize";

pub async fn token(api: &Api, scope: &str) -> String {
    let client = format!("card_test_{}", new_id());
    let secret = "synthetic-card-test-secret";
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/counterparty_clients",
        &new_id(),
        None,
        Some(json!({"counterparty_clients": [{
            "client_id": client, "client_secret": secret, "institution_code": "901", "scope": scope
        }]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = form_call(
        &api.generator,
        "/oauth/2.0/token",
        format!(
            "client_id={client}&client_secret={secret}&scope={scope}&grant_type=client_credentials"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scope"], scope);
    body["access_token"].as_str().unwrap().to_owned()
}

pub async fn card(api: &Api, account: &str, status: &str) -> String {
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/cards",
        &new_id(),
        None,
        Some(json!({"cards": [{"account_no": account, "status": status}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["cards"][0]["card_no"].as_str().unwrap().to_owned()
}

pub fn message(card: &str, key: &str, amount: &str) -> Value {
    json!({
        "idempotency_key": key,
        "card_no": card,
        "amount": amount,
        "currency": "KRW",
        "merchant_name": " 테스트 가게 ",
        "mcc": "0123",
        "country": "KR",
        "entry_mode": "online",
    })
}

pub async fn send(
    api: &Api,
    token: Option<&str>,
    request_id: &str,
    message: Value,
) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        ENDPOINT,
        request_id,
        token,
        Some(message),
    )
    .await
}
