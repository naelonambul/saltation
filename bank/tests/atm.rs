//! ATM contract proof, independent of the database and runnable in the Oracle harness.

use serde_json::Value;

#[test]
fn contracts_are_valid_openapi() {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".to_owned());
    for name in ["atm", "internal"] {
        let text =
            std::fs::read_to_string(format!("{dir}/openapi/{name}.yaml")).expect("read contract");
        let doc: Value = serde_yaml::from_str(&text).expect("contract YAML");
        assert!(
            doc["openapi"]
                .as_str()
                .expect("OpenAPI version")
                .starts_with("3.")
        );
        assert_refs_resolve(&doc, &doc);
        for path in doc["paths"].as_object().expect("paths").values() {
            for method in [
                "get", "post", "put", "patch", "delete", "options", "head", "trace",
            ] {
                if let Some(operation) = path.get(method) {
                    assert!(
                        !operation["responses"]
                            .as_object()
                            .expect("responses")
                            .is_empty()
                    );
                }
            }
        }
        let defs = doc["components"]["schemas"]
            .to_string()
            .replace("#/components/schemas/", "#/$defs/");
        let defs: Value = serde_json::from_str(&defs).expect("defs");
        for schema_name in defs.as_object().expect("schemas").keys() {
            let schema = serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": defs,
                "$ref": format!("#/$defs/{schema_name}")
            });
            jsonschema::draft202012::new(&schema).expect("valid component schema");
        }
    }
}

fn assert_refs_resolve(value: &Value, doc: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref") {
                let reference = reference.as_str().expect("string reference");
                let pointer = reference.strip_prefix('#').expect("local reference");
                assert!(doc.pointer(pointer).is_some(), "unresolved {reference}");
            }
            for child in object.values() {
                assert_refs_resolve(child, doc);
            }
        }
        Value::Array(array) => {
            for child in array {
                assert_refs_resolve(child, doc);
            }
        }
        _ => {}
    }
}

mod common;
use axum::http::StatusCode;
use bank::shared::auth;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{Api, TEST_ATM_PIN, assert_no_pin, call, find_pin, new_id};
use serde_json::json;

const TERMINAL: SqlStmt = sql_stmt!("test.atm_terminal", "sql/atm_terminal.sql");
const ACCOUNT_COUNT: SqlStmt = sql_stmt!("test.atm_account_count", "sql/atm_account_count.sql");
const CREDENTIAL: SqlStmt = sql_stmt!("test.atm_credential", "sql/atm_credential.sql");
const EVENTS: SqlStmt = sql_stmt!("test.atm_events", "sql/atm_events.sql");
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn live() -> Api {
    let api = common::api().await;
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/mode/live",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    api
}
fn terminal(id: &str) -> Value {
    json!({"terminal_id": id, "region": "KR-11", "location": "서울", "latitude": "37.566500", "longitude": "126.978000"})
}
async fn seed_terminal(api: &Api, rows: Vec<Value>) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        "/internal/v1/seed/atm_terminals",
        &new_id(),
        None,
        Some(json!({"atm_terminals": rows})),
    )
    .await
}
async fn terminal_rows(api: &Api, id: &str) -> Vec<(String, String)> {
    let id = id.to_owned();
    api.state
        .db
        .run(move |s| s.query_all(&TERMINAL, &[&id]))
        .await
        .unwrap()
}
async fn seed_account(api: &Api, rows: Vec<Value>) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        "/internal/v1/seed/accounts",
        &new_id(),
        None,
        Some(json!({"accounts": rows})),
    )
    .await
}
async fn credential(api: &Api, account: &str) -> (String, i64, Option<String>) {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&CREDENTIAL, &[&account]))
        .await
        .unwrap()
}
async fn account_count(api: &Api, account: &str) -> i64 {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&ACCOUNT_COUNT, &[&account]))
        .await
        .unwrap()
}

#[tokio::test]
async fn seeding_terminals_validates_every_field() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let invalid = [
        ("region", "KR-10".to_owned()),
        ("region", "KR-51".to_owned()),
        ("region", "kr-11".to_owned()),
        ("latitude", "32.999999".to_owned()),
        ("latitude", "38.700001".to_owned()),
        ("latitude", "37.1234567".to_owned()),
        ("latitude", "1e1".to_owned()),
        ("latitude", "abc".to_owned()),
        ("longitude", "124.499999".to_owned()),
        ("longitude", "132.000001".to_owned()),
        ("location", "x".repeat(61)),
        ("terminal_id", "x".repeat(21)),
        ("location", String::new()),
        ("terminal_id", String::new()),
        ("latitude", "+37.5".to_owned()),
        ("latitude", "37.".to_owned()),
        ("latitude", ".5".to_owned()),
    ];
    for (field, value) in invalid {
        let id = format!("T{}", common::unique_digits(15));
        let companion = format!("T{}", common::unique_digits(15));
        let mut row = terminal(&id);
        row[field] = json!(value);
        let rejected_id = row["terminal_id"].as_str().unwrap().to_owned();
        let (status, body) = seed_terminal(&api, vec![terminal(&companion), row]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {body}");
        assert!(terminal_rows(&api, &rejected_id).await.is_empty());
        assert!(terminal_rows(&api, &companion).await.is_empty());
    }
    let id = format!("T{}", common::unique_digits(15));
    let (status, body) = seed_terminal(&api, vec![terminal(&id)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_contract("SeedAtmTerminalsResponse", &body);
    assert_eq!(body["atm_terminals"], json!([{"terminal_id": id}]));
    assert_eq!(
        terminal_rows(&api, &id).await,
        vec![("37.566500".into(), "126.978000".into())]
    );
    let (status, body) = seed_terminal(&api, vec![terminal(&id)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["atm_terminals"], json!([]));
}

#[tokio::test]
async fn seeding_pins_validates_and_hashes() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    // The shared helper seeds a valid customer and demand account.
    let customer = common::customer(&api, 10000).await;
    // A real customer, so that only the PIN can be the reason for the 400.
    let no = format!("C{}", common::unique_digits(10));
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None,
        Some(json!({"customers": [{"customer_no": no, "name": "ATM 고객", "phone": format!("010{}", common::unique_digits(8)), "customer_type": "INDIVIDUAL", "password": "atm-test-password"}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for pin in [Some("123"), Some("12345"), Some("12a4"), None] {
        let account = format!("95{}", common::unique_digits(10));
        let companion = format!("95{}", common::unique_digits(10));
        let mut row = json!({"customer_no": no, "account_no": account});
        if let Some(pin) = pin {
            row["atm_pin"] = json!(pin);
        }
        let (status, body) = seed_account(
            &api,
            vec![
                json!({"customer_no": no, "account_no": companion, "atm_pin": TEST_ATM_PIN}),
                row,
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(account_count(&api, &account).await, 0);
        assert_eq!(account_count(&api, &companion).await, 0);
    }
    let (hash, failed, locked) = credential(&api, &customer.account).await;
    assert!(hash.starts_with("$argon2id$"));
    assert!(
        auth::verify_secret(hash.clone(), TEST_ATM_PIN.into())
            .await
            .unwrap()
    );
    assert!(!auth::verify_secret(hash, "0000".into()).await.unwrap());
    assert_eq!(failed, 0);
    assert!(locked.is_none());
}

#[tokio::test]
async fn seeding_leaks_no_pin() {
    let _guard = TEST_LOCK.lock().await;
    // Keep every line emitted during this test, including initialization.
    let start = common::logs().len();
    let api = live().await;
    let no = format!("C{}", common::unique_digits(10));
    let account = format!("94{}", common::unique_digits(10));
    let (status, customer_response) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None,
        Some(json!({"customers": [{"customer_no": no, "name": "ATM 고객", "phone": format!("010{}", common::unique_digits(8)), "customer_type": "INDIVIDUAL", "password": "atm-test-password"}]}))).await;
    assert_eq!(status, StatusCode::OK, "{customer_response}");
    let (status, account_response) = seed_account(&api, vec![json!({"customer_no": no, "account_no": account, "atm_pin": TEST_ATM_PIN, "opening_balance": 10000})]).await;
    assert_eq!(status, StatusCode::OK, "{account_response}");
    let (status, terminal_response) = seed_terminal(
        &api,
        vec![terminal(&format!("T{}", common::unique_digits(15)))],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{terminal_response}");
    let (hash, _, _) = credential(&api, &account).await;
    let rows: Vec<String> = api
        .state
        .db
        .run(|s| s.query_all(&EVENTS, &[]))
        .await
        .unwrap();
    assert!(!rows.is_empty(), "opening balance emits an event");
    let events: Vec<Value> = rows
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let responses = vec![customer_response, account_response, terminal_response];
    let lines = common::logs();
    let lines = &lines[start..];
    for (what, values) in [
        ("responses", responses.as_slice()),
        ("logs", lines),
        ("outbox", events.as_slice()),
    ] {
        assert_no_pin(what, values, TEST_ATM_PIN);
        for value in values {
            let text = value.to_string();
            assert!(!text.contains(&hash), "{what} contains stored PIN hash");
            assert!(
                !text.contains("$argon2id$"),
                "{what} contains an Argon2id hash"
            );
        }
    }
}

#[tokio::test]
async fn secret_checks_catch_a_planted_pin() {
    let _guard = TEST_LOCK.lock().await;
    let _api = live().await;
    tracing::info!(note = "pin 7391 here", "planted pin");
    let lines = common::logs();
    assert!(find_pin(&json!(lines), TEST_ATM_PIN).is_some());
    let event = json!({"payload": {"x": "7391"}});
    assert!(find_pin(&event, TEST_ATM_PIN).is_some());
    for values in [vec![json!({"note": "pin 7391 here"})], vec![event]] {
        assert!(
            std::panic::catch_unwind(|| assert_no_pin("planted", &values, TEST_ATM_PIN)).is_err()
        );
    }
    for value in ["17391", "7391.5", "73910"] {
        assert!(find_pin(&json!(value), TEST_ATM_PIN).is_none());
    }
    assert!(find_pin(&json!(7391), TEST_ATM_PIN).is_some());
}

#[tokio::test]
async fn captured_logs_include_any_line() {
    let _guard = TEST_LOCK.lock().await;
    let _api = live().await;
    let message = format!("planted-line-{}", new_id());
    tracing::info!("{message}");
    assert!(
        common::logs()
            .iter()
            .any(|v| v["message"].as_str() == Some(&message))
    );
}
