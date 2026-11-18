//! Card network contract, persisted outcomes, concurrency and fail-closed payment effects.
mod common;

use std::sync::OnceLock;

use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::card_network::{ENDPOINT, card, message, send, token};
use common::{Api, api, call, customer, new_id};
use serde_json::{Value, json};

const REQUEST: SqlStmt = sql_stmt!("test.card_payment_request", "sql/card_payment_request.sql");
const ATTRIBUTES: SqlStmt = sql_stmt!(
    "test.card_payment_attributes",
    "sql/card_payment_attributes.sql"
);
const EVENTS: SqlStmt = sql_stmt!("test.card_payment_events", "sql/card_payment_events.sql");
const ACCOUNT: SqlStmt = sql_stmt!("test.card_payment_account", "sql/card_payment_account.sql");
const STATUS: SqlStmt = sql_stmt!(
    "test.card_payment_account_status",
    "sql/card_payment_account_status.sql"
);
const FAULT_ON: SqlStmt = sql_stmt!(
    "test.card_payment_fault_on",
    "sql/card_payment_fault_on.sql"
);
const FAULT_OFF: SqlStmt = sql_stmt!(
    "test.card_payment_fault_off",
    "sql/card_payment_fault_off.sql"
);

async fn rows(api: &Api, stmt: SqlStmt, key: &str) -> Vec<Value> {
    let key = key.to_owned();
    let rows: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&stmt, &[&key]))
        .await
        .unwrap();
    rows.iter()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect()
}

async fn one(api: &Api, stmt: SqlStmt, key: &str) -> Value {
    let values = rows(api, stmt, key).await;
    assert_eq!(values.len(), 1, "{values:?}");
    values.into_iter().next().unwrap()
}

async fn set_status(api: &Api, account: &str, status: &'static str) {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| {
            s.execute(&STATUS, &[&status, &account])?;
            s.commit()
        })
        .await
        .unwrap();
}

fn contract() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".into());
        serde_yaml::from_str(&std::fs::read_to_string(format!("{dir}/openapi/card.yaml")).unwrap())
            .unwrap()
    })
}

fn check_schema(schema: &Value, value: &Value) {
    let root: Value = serde_json::from_str(
        &json!({
            "$defs": contract()["components"],
            "allOf": [schema]
        })
        .to_string()
        .replace("#/components/", "#/$defs/"),
    )
    .unwrap();
    let validator = jsonschema::draft202012::new(&root).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?} in {value}");
}

fn check_response(id: &str, response: (StatusCode, Value), expected: u16) -> Value {
    let (status, body) = response;
    assert_eq!(status.as_u16(), expected, "{body}");
    assert_eq!(body["request_id"], id);
    let schema = &contract()["paths"][ENDPOINT]["post"]["responses"][expected.to_string()]["content"]
        ["application/json"]["schema"];
    assert!(schema.is_object(), "missing schema for {expected}");
    check_schema(schema, &body);
    body
}

async fn events(api: &Api, id: &str, domain: Option<&str>) -> Vec<Value> {
    let all = rows(api, EVENTS, id).await;
    let mut completed = 0;
    let mut decisions = 0;
    for event in &all {
        assert_eq!(event["request_id"], id);
        assert_eq!(event["channel"], "CARD");
        assert!(event["session_id"].is_null());
        match event["event_type"].as_str().unwrap() {
            "request.completed" => completed += 1,
            "card.authorized" | "card.declined" => {
                assert_eq!(event["event_type"].as_str(), domain);
                decisions += 1;
            }
            "account.status_changed" => {}
            other => panic!("unexpected card request event {other}"),
        }
    }
    assert_eq!(completed, 1, "{all:?}");
    assert_eq!(decisions, usize::from(domain.is_some()), "{all:?}");
    all
}

async fn unchanged_attempt(api: &Api, id: &str) -> Value {
    let row = one(api, REQUEST, id).await;
    for key in ["authorization", "transfers", "legs"] {
        assert_eq!(row[key], 0, "{row}");
    }
    assert!(row["bank_id"].is_null());
    assert!(row["key"].is_null());
    row
}

#[tokio::test]
async fn validates_scope_listener_and_payload_before_payment() {
    let api = api().await;
    let card_token = token(&api, "card").await;
    let wrong = token(&api, "oob").await;
    let number = bank::features::cards::random_number().unwrap();
    let valid = message(&number, "scope-check", "1");

    let (status, _) = call(
        &api.public,
        "POST",
        ENDPOINT,
        &new_id(),
        Some(&card_token),
        Some(valid.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for (bearer, expected, code) in [
        (None, 401, "O0001"),
        (Some(wrong.as_str()), 403, "O0003"),
        (Some("invalid-token"), 401, "O0001"),
    ] {
        let id = new_id();
        let body = check_response(&id, send(&api, bearer, &id, valid.clone()).await, expected);
        assert_eq!(body["rsp_code"], code);
        let row = unchanged_attempt(&api, &id).await;
        assert_eq!(row["kind"], "CARD_PAYMENT");
        assert_eq!(row["code"], code);
        events(&api, &id, None).await;
    }

    for (field, input) in [
        ("card_no", json!("4111111111111111")),
        ("currency", json!("GBP")),
        ("amount", json!("0")),
        ("latitude", json!("0")),
        ("country", json!("ZZ")),
    ] {
        let mut body = valid.clone();
        body[field] = input;
        let id = new_id();
        check_response(&id, send(&api, Some(&card_token), &id, body).await, 400);
        assert!(rows(&api, REQUEST, &id).await.is_empty());
        events(&api, &id, None).await;
    }
}

#[tokio::test]
async fn converts_exactly_and_posts_one_customer_initiated_debit() {
    let api = api().await;
    let token = token(&api, "card").await;
    let account = customer(&api, 100_000).await.account;
    let card = card(&api, &account, "ACTIVE").await;
    set_status(&api, &account, "DORMANT").await;
    let before = one(&api, ACCOUNT, &account).await;
    let merchant = format!(" {} ", "가".repeat(58));
    let mut balance = 100_000;
    for (currency, orig, krw) in [
        ("KRW", "001", 1),
        ("USD", "2", 2600),
        ("EUR", "3", 4200),
        ("JPY", "200", 1800),
        ("CNY", "2", 400),
    ] {
        let id = new_id();
        let mut req = message(&card, &id, orig);
        req["currency"] = json!(currency);
        req["entry_mode"] = json!("in_person");
        req["latitude"] = json!("-90.000000");
        req["longitude"] = json!("180");
        req["merchant_name"] = json!(merchant);
        check_schema(&json!({"$ref":"#/components/schemas/Authorization"}), &req);
        let response = check_response(&id, send(&api, Some(&token), &id, req).await, 200);
        assert_eq!(response["krw_amount"], krw.to_string());
        balance -= krw;
        let stored = one(&api, ATTRIBUTES, &id).await;
        assert_eq!(stored["merchant"], merchant);
        assert_eq!(stored["counterparty"], merchant);
        assert_eq!(stored["card_no"], card);
        assert_eq!(stored["amount"], orig.parse::<u64>().unwrap());
        assert_eq!(stored["currency"], currency);
        assert_eq!(stored["entry"], "in_person");
        assert_eq!(stored["mcc"], "0123");
        assert_eq!(stored["country"], "KR");
        assert_eq!(stored["kind"], "card_payment");
        assert_eq!(stored["leg"], -krw);
        assert_eq!(stored["balance"], balance);
        assert_eq!(stored["channel"], "CARD");
        assert_eq!(stored["inout"], "출금");
        assert_eq!(stored["latitude_micro"], -90_000_000);
        assert_eq!(stored["longitude_micro"], 180_000_000);
        assert!(stored["bank"].is_null() && stored["counterparty_account"].is_null());
        let request = one(&api, REQUEST, &id).await;
        assert_eq!(
            (
                request["authorization"].as_i64(),
                request["transfers"].as_i64(),
                request["legs"].as_i64()
            ),
            (Some(1), Some(1), Some(1))
        );
        assert_eq!(request["key"], id);
        assert!(request["bank_id"].is_null() && request["session_id"].is_null());
        let all = events(&api, &id, Some("card.authorized")).await;
        let domain = all
            .iter()
            .find(|e| e["event_type"] == "card.authorized")
            .unwrap();
        assert_eq!(domain["payload"]["amount"], orig);
        assert_eq!(domain["payload"]["merchant_name"], merchant);
        assert_eq!(domain["payload"]["latitude"], "-90.000000");
        assert_eq!(domain["account_no"], account);
    }
    let after = one(&api, ACCOUNT, &account).await;
    assert_eq!(after["balance"], balance);
    assert_eq!(
        after["sequence"].as_i64(),
        before["sequence"].as_i64().map(|n| n + 5)
    );
    assert_eq!(after["status"], "ACTIVE");
    assert_eq!(after["reactivations"], 1);
    assert!(after["activity"].is_string());
    assert_eq!(after["limits"], before["limits"]);
}

#[tokio::test]
async fn stores_all_declines_and_replays_the_original_decision() {
    let api = api().await;
    let token = token(&api, "card").await;
    let account = customer(&api, 10).await.account;
    let active = card(&api, &account, "ACTIVE").await;
    let blocked = card(&api, &account, "BLOCKED").await;
    let unknown = bank::features::cards::random_number().unwrap();
    set_status(&api, &account, "DORMANT").await;
    for (card, reason, code) in [
        (unknown, "unknown_card", "404"),
        (blocked, "blocked_card", "403"),
        (active.clone(), "insufficient_balance", "405"),
        (active, "closed_account", "403"),
    ] {
        if reason == "closed_account" {
            set_status(&api, &account, "CLOSED").await;
        }
        let before = one(&api, ACCOUNT, &account).await;
        let id = new_id();
        let req = message(&card, &id, "20");
        let body = check_response(&id, send(&api, Some(&token), &id, req.clone()).await, 422);
        assert_eq!(body["decline_reason"], reason);
        assert_eq!(body["bank_rsp_code"], code);
        assert_eq!(body["krw_amount"], "20");
        let row = one(&api, REQUEST, &id).await;
        assert_eq!(row["authorization"], 1);
        assert_eq!(row["transfers"], 0);
        assert_eq!(row["legs"], 0);
        assert_eq!(row["status"], "REJECTED");
        assert_eq!(row["bank_code"], code);
        let stored = one(&api, ATTRIBUTES, &id).await;
        assert_eq!(stored["card_no"], card);
        assert_eq!(stored["card_id"].is_null(), reason == "unknown_card");
        assert!(stored["latitude_micro"].is_null() && stored["longitude_micro"].is_null());
        let all = events(&api, &id, Some("card.declined")).await;
        if reason == "unknown_card" {
            assert!(
                all.iter()
                    .all(|e| e["account_no"].is_null() && e["customer_no"].is_null())
            );
        }
        let replay = new_id();
        let repeated = check_response(&replay, send(&api, Some(&token), &replay, req).await, 422);
        assert_eq!(repeated["decline_reason"], reason);
        assert_eq!(repeated["krw_amount"], "20");
        assert_eq!(repeated["bank_rsp_code"], code);
        assert!(rows(&api, REQUEST, &replay).await.is_empty());
        assert_eq!(
            events(&api, &replay, None).await[0]["payload"]["replayed"],
            true
        );
        assert_eq!(one(&api, ACCOUNT, &account).await, before);
    }
}

#[tokio::test]
async fn scopes_long_keys_and_never_debits_twice_on_concurrent_replay() {
    let api = api().await;
    let first = token(&api, "card").await;
    let second = token(&api, "card").await;
    let account = customer(&api, 65).await.account;
    let card = card(&api, &account, "ACTIVE").await;
    let key = format!("card-payment-{}", new_id());
    assert_eq!(key.len(), 39);
    let key = format!("{key}X");
    let req = message(&card, &key, "20");
    let a = new_id();
    let b = new_id();
    let (ra, rb) = tokio::join!(
        send(&api, Some(&first), &a, req.clone()),
        send(&api, Some(&first), &b, req.clone())
    );
    let a_body = check_response(&a, ra, 200);
    let b_body = check_response(&b, rb, 200);
    assert_eq!(a_body["krw_amount"], b_body["krw_amount"]);
    let (winner, replay) = if rows(&api, REQUEST, &a).await.is_empty() {
        (&b, &a)
    } else {
        (&a, &b)
    };
    let original = one(&api, REQUEST, winner).await;
    assert_eq!(original["key"], key);
    assert!(original["bank_id"].is_null());
    assert_eq!(original["legs"], 1);
    assert!(rows(&api, REQUEST, replay).await.is_empty());
    events(&api, winner, Some("card.authorized")).await;
    events(&api, replay, None).await;

    let conflict = new_id();
    let other = message(&card, &key, "21");
    check_response(
        &conflict,
        send(&api, Some(&first), &conflict, other).await,
        409,
    );
    assert_eq!(unchanged_attempt(&api, &conflict).await["code"], "A0007");
    events(&api, &conflict, None).await;
    assert_eq!(one(&api, REQUEST, winner).await, original);
    assert_eq!(one(&api, ACCOUNT, &account).await["balance"], 45);

    let independent = new_id();
    check_response(
        &independent,
        send(&api, Some(&second), &independent, req.clone()).await,
        200,
    );
    assert_eq!(one(&api, ACCOUNT, &account).await["balance"], 25);

    // A reused request ID cannot claim a different key, even with matching attributes.
    let different = message(&card, "new-key-same-request-id", "20");
    check_response(
        winner,
        send(&api, Some(&first), winner, different).await,
        409,
    );
    let combined = rows(&api, REQUEST, winner).await;
    assert_eq!(combined.len(), 2);
    assert_eq!(combined[0], original);
    assert_eq!(combined[1]["reused_id"], winner.as_str());
    assert_eq!(combined[1]["code"], "A0007");

    let c = new_id();
    let d = new_id();
    let (rc, rd) = tokio::join!(
        send(&api, Some(&first), &c, message(&card, &c, "20")),
        send(&api, Some(&first), &d, message(&card, &d, "20"))
    );
    let mut statuses = [rc.0.as_u16(), rd.0.as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 422]);
    assert_eq!(one(&api, ACCOUNT, &account).await["balance"], 5);
}

#[tokio::test]
async fn outbox_write_failure_rolls_back_the_entire_payment() {
    let api = api().await;
    let token = token(&api, "card").await;
    let account = customer(&api, 100).await.account;
    let number = card(&api, &account, "ACTIVE").await;
    let before = one(&api, ACCOUNT, &account).await;
    let id = new_id();
    let mut request = message(&number, &id, "43");
    request["merchant_name"] = json!("__card_outbox_fault__");

    api.state
        .db
        .run(|s| s.execute(&FAULT_ON, &[]))
        .await
        .unwrap();
    let failed = send(&api, Some(&token), &id, request.clone()).await;
    api.state
        .db
        .run(|s| s.execute(&FAULT_OFF, &[]))
        .await
        .unwrap();

    let body = check_response(&id, failed, 503);
    assert_eq!(body["rsp_code"], "A0013");
    assert!(rows(&api, REQUEST, &id).await.is_empty());
    assert!(rows(&api, ATTRIBUTES, &id).await.is_empty());
    assert_eq!(one(&api, ACCOUNT, &account).await, before);
    events(&api, &id, None).await;

    let retry = new_id();
    check_response(&retry, send(&api, Some(&token), &retry, request).await, 200);
    assert_eq!(one(&api, ACCOUNT, &account).await["balance"], 57);
    assert_eq!(one(&api, REQUEST, &retry).await["legs"], 1);
    events(&api, &retry, Some("card.authorized")).await;
}
