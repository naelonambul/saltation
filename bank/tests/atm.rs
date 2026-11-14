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

const SESSIONS: SqlStmt = sql_stmt!("test.atm_sessions", "sql/atm_sessions.sql");
const REQUESTS: SqlStmt = sql_stmt!("test.atm_requests", "sql/atm_requests.sql");
const REQUEST_EVENTS: SqlStmt = sql_stmt!("test.atm_request_events", "sql/atm_request_events.sql");
const SESSION_EVENTS: SqlStmt = sql_stmt!("test.atm_session_events", "sql/atm_session_events.sql");
const DELETE_CREDENTIAL: SqlStmt = sql_stmt!(
    "test.atm_delete_credential",
    "sql/atm_delete_credential.sql"
);
const CLOSE_ACCOUNT: SqlStmt = sql_stmt!("test.atm_close_account", "sql/atm_close_account.sql");
const EXPIRE_SESSION: SqlStmt = sql_stmt!("test.atm_expire_session", "sql/atm_expire_session.sql");
async fn fixture(api: &Api) -> (String, common::Customer) {
    let terminal = format!("T{}", common::unique_digits(15));
    assert_eq!(
        seed_terminal(api, vec![self::terminal(&terminal)]).await.0,
        StatusCode::OK
    );
    (terminal, common::customer(api, 10000).await)
}
async fn rows(api: &Api, stmt: SqlStmt, key: &str) -> Vec<Value> {
    let key = key.to_owned();
    let result: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&stmt, &[&key]))
        .await
        .unwrap();
    result
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
async fn start_session(
    api: &Api,
    terminal: &str,
    account: &str,
    pin: &str,
    id: &str,
) -> (StatusCode, Value) {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let req = Request::post("/atm/v1/session")
        .header("content-type", "application/json")
        .header("x-request-id", id)
        .header("x-sim-ip", "192.0.2.53")
        .header("x-sim-region", "KR-26")
        .header("x-sim-device", "atm-test")
        .body(Body::from(
            json!({"terminal_id": terminal, "account_no": account, "pin": pin}).to_string(),
        ))
        .unwrap();
    let res = api.generator.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn at(value: &Value) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(value.as_str().unwrap()).unwrap()
}
fn check_location(e: &Value, terminal: &str, account: &str, request: &str) {
    assert_eq!(e["channel"], "ATM");
    assert_eq!(e["account_no"], account);
    for (key, value) in [
        ("terminal_id", terminal),
        ("region", "KR-11"),
        ("latitude", "37.566500"),
        ("longitude", "126.978000"),
        ("account_no", account),
        ("request_id", request),
    ] {
        assert_eq!(e["payload"][key], value, "{key}");
    }
    assert!(common::envelope_errors(e).is_empty());
}
async fn finish_session(api: &Api, token: &str, id: &str) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        "/atm/v1/session/end",
        id,
        Some(token),
        None,
    )
    .await
}
#[tokio::test]
async fn session_start_and_end() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let id = new_id();
    let (status, body) = start_session(&api, &terminal, &customer.account, TEST_ATM_PIN, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_atm_contract("SessionResponse", &body);
    let token = body["atm_session_token"].as_str().unwrap();
    let sessions = rows(&api, SESSIONS, &terminal).await;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session["token_hash"], auth::token_hash(token));
    assert_eq!(
        at(&session["expires_at"]) - at(&session["started_at"]),
        chrono::Duration::seconds(180)
    );
    assert_eq!(at(&body["expires_at"]), at(&session["expires_at"]));
    let events = rows(&api, REQUEST_EVENTS, &id).await;
    assert_eq!(
        events
            .iter()
            .map(|e| e["event_type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["atm.session_started", "request.completed"]
    );
    check_location(&events[0], &terminal, &customer.account, &id);
    assert_eq!(events[1]["payload"]["http_status"], 200);
    let end_id = new_id();
    let (status, body) = finish_session(&api, token, &end_id).await;
    assert_eq!(status, StatusCode::OK);
    common::assert_atm_contract("Envelope", &body);
    let ended = rows(&api, SESSIONS, &terminal).await;
    assert_eq!(ended[0]["end_reason"], "ended");
    let session_id = session["session_id"].as_str().unwrap();
    let ends = rows(&api, SESSION_EVENTS, session_id).await;
    assert_eq!(ends.len(), 1);
    check_location(&ends[0], &terminal, &customer.account, &end_id);
    let second_id = new_id();
    let (status, body) = finish_session(&api, token, &second_id).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["rsp_code"], "O0001");
    assert_eq!(rows(&api, SESSION_EVENTS, session_id).await.len(), 1);
    let events = rows(&api, REQUEST_EVENTS, &second_id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event_type"], "request.completed");
    assert_eq!(events[0]["payload"]["http_status"], 401);
}
async fn check_refusal(
    api: &Api,
    terminal: &str,
    account: &str,
    pin: &str,
    status: StatusCode,
    bank: Option<&str>,
    exists: bool,
) {
    let id = new_id();
    let (actual, body) = start_session(api, terminal, account, pin, &id).await;
    assert_eq!(actual, status, "{body}");
    let code = if bank.is_some() { "A0004" } else { "A0003" };
    assert_eq!(body["rsp_code"], code);
    assert_eq!(body.get("bank_rsp_code").and_then(Value::as_str), bank);
    let requests = rows(api, REQUESTS, &id).await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request["status"], "REJECTED");
    assert_eq!(request["request_type"], "ATM_SESSION");
    assert_eq!(request["channel"], "ATM");
    assert_eq!(request["rsp_code"], code);
    assert_eq!(request["bank_rsp_code"], json!(bank));
    assert_eq!(
        request["terminal_id"],
        if exists { json!(terminal) } else { Value::Null }
    );
    assert!(request["idem_scope"].is_null());
    assert!(request["idem_key"].is_null());
    assert_eq!(request["ip"], "192.0.2.53");
    assert_eq!(request["region"], "KR-26");
    assert_eq!(request["device_id"], "atm-test");
    let events = rows(api, REQUEST_EVENTS, &id).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["event_type"] == "request.completed")
            .count(),
        1
    );
    let rejected: Vec<_> = events
        .iter()
        .filter(|e| e["event_type"] == "atm.session_rejected")
        .collect();
    assert_eq!(rejected.len(), usize::from(exists));
    if exists {
        check_location(rejected[0], terminal, account, &id);
    }
}
#[tokio::test]
async fn every_refusal_is_stored() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    check_refusal(
        &api,
        &format!("U{}", common::unique_digits(15)),
        &customer.account,
        TEST_ATM_PIN,
        StatusCode::NOT_FOUND,
        None,
        false,
    )
    .await;
    check_refusal(
        &api,
        &terminal,
        &format!("99{}", common::unique_digits(12)),
        TEST_ATM_PIN,
        StatusCode::UNPROCESSABLE_ENTITY,
        Some("404"),
        true,
    )
    .await;
    let no_pin = common::customer(&api, 0).await;
    let account = no_pin.account.clone();
    api.state
        .db
        .run(move |s| {
            s.execute(&DELETE_CREDENTIAL, &[&account])?;
            s.commit()
        })
        .await
        .unwrap();
    check_refusal(
        &api,
        &terminal,
        &no_pin.account,
        TEST_ATM_PIN,
        StatusCode::UNPROCESSABLE_ENTITY,
        Some("404"),
        true,
    )
    .await;
    let closed = common::customer(&api, 0).await;
    let account = closed.account.clone();
    api.state
        .db
        .run(move |s| {
            s.execute(&CLOSE_ACCOUNT, &[&account])?;
            s.commit()
        })
        .await
        .unwrap();
    check_refusal(
        &api,
        &terminal,
        &closed.account,
        TEST_ATM_PIN,
        StatusCode::UNPROCESSABLE_ENTITY,
        Some("403"),
        true,
    )
    .await;
    check_refusal(
        &api,
        &terminal,
        &customer.account,
        "0000",
        StatusCode::UNPROCESSABLE_ENTITY,
        Some("410"),
        true,
    )
    .await;
}
#[tokio::test]
async fn lock_after_three_wrong_pins() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let before = api
        .state
        .clock
        .state()
        .await
        .unwrap()
        .business_time
        .with_timezone(&bank::shared::clock::kst());
    for i in 0..3 {
        let id = new_id();
        let (status, body) = start_session(&api, &terminal, &customer.account, "0000", &id).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["bank_rsp_code"], "410");
        let events = rows(&api, REQUEST_EVENTS, &id).await;
        assert_eq!(events[0]["payload"]["locked"], i == 2);
    }
    let (_, failed, locked) = credential(&api, &customer.account).await;
    assert_eq!(failed, 3);
    assert_eq!(
        locked.unwrap(),
        before
            .date_naive()
            .succ_opt()
            .unwrap()
            .format("%Y%m%d000000")
            .to_string()
    );
    for pin in [TEST_ATM_PIN, "0000"] {
        check_refusal(
            &api,
            &terminal,
            &customer.account,
            pin,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("409"),
            true,
        )
        .await;
    }
}
async fn concurrent_starts(
    api: &Api,
    terminal: &str,
    account: &str,
    pin: &str,
) -> Vec<(String, StatusCode, Value)> {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let api = Api {
            generator: api.generator.clone(),
            public: api.public.clone(),
            state: api.state.clone(),
        };
        let (terminal, account, pin) = (terminal.to_owned(), account.to_owned(), pin.to_owned());
        tasks.spawn(async move {
            let id = new_id();
            let (status, body) = start_session(&api, &terminal, &account, &pin, &id).await;
            (id, status, body)
        });
    }
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result.unwrap());
    }
    results
}
#[tokio::test]
async fn concurrent_wrong_pins_lock_once() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let results = concurrent_starts(&api, &terminal, &customer.account, "0000").await;
    for (_, status, _) in &results {
        assert_eq!(*status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(
        results
            .iter()
            .filter(|(_, _, b)| b["bank_rsp_code"] == "410")
            .count(),
        3
    );
    assert_eq!(
        results
            .iter()
            .filter(|(_, _, b)| b["bank_rsp_code"] == "409")
            .count(),
        5
    );
    let mut locked = 0;
    for (id, _, _) in results {
        locked += rows(&api, REQUEST_EVENTS, &id)
            .await
            .iter()
            .filter(|e| e["event_type"] == "atm.session_rejected" && e["payload"]["locked"] == true)
            .count();
    }
    assert_eq!(locked, 1);
    assert_eq!(credential(&api, &customer.account).await.1, 3);
}
#[tokio::test]
async fn concurrent_starts_replace() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let results = concurrent_starts(&api, &terminal, &customer.account, TEST_ATM_PIN).await;
    for (_, status, body) in results {
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let sessions = rows(&api, SESSIONS, &terminal).await;
    assert_eq!(sessions.len(), 8);
    assert_eq!(
        sessions.iter().filter(|s| s["ended_at"].is_null()).count(),
        1
    );
    assert_eq!(
        sessions
            .iter()
            .filter(|s| s["end_reason"] == "replaced")
            .count(),
        7
    );
    for session in sessions {
        let events = rows(
            &api,
            SESSION_EVENTS,
            session["session_id"].as_str().unwrap(),
        )
        .await;
        assert!(events.len() <= 1);
        if session["end_reason"] == "replaced" {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["payload"]["session_id"], session["session_id"]);
            assert_eq!(
                events[0]["payload"]["start_request_id"],
                session["request_id"]
            );
            assert_ne!(events[0]["request_id"], session["request_id"]);
        }
    }
}
#[tokio::test]
async fn no_token_or_pin_leaks() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let begin = common::logs().len();
    let (status, mut started) =
        start_session(&api, &terminal, &customer.account, TEST_ATM_PIN, &new_id()).await;
    assert_eq!(status, StatusCode::OK);
    let token = started["atm_session_token"].as_str().unwrap().to_owned();
    let ended = finish_session(&api, &token, &new_id()).await.1;
    let wrong = start_session(&api, &terminal, &customer.account, "0000", &new_id())
        .await
        .1;
    assert_eq!(
        started.as_object_mut().unwrap().remove("atm_session_token"),
        Some(json!(token))
    );
    let responses = vec![started, ended, wrong];
    let logs = common::logs();
    let raw: Vec<String> = api
        .state
        .db
        .run(|s| s.query_all(&EVENTS, &[]))
        .await
        .unwrap();
    let events: Vec<Value> = raw
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let hash = credential(&api, &customer.account).await.0;
    for (what, values) in [
        ("responses", responses.as_slice()),
        ("logs", &logs[begin..]),
        ("outbox", events.as_slice()),
    ] {
        assert_no_pin(what, values, TEST_ATM_PIN);
        for value in values {
            let text = value.to_string();
            assert!(!text.contains(&token), "{what} contains session token");
            assert!(!text.contains(&hash), "{what} contains PIN hash");
            assert!(!text.contains("$argon2id$"), "{what} contains PIN hash");
        }
    }
}
#[tokio::test]
async fn presented_expired_session_ends_once() {
    let _guard = TEST_LOCK.lock().await;
    let api = live().await;
    let (terminal, customer) = fixture(&api).await;
    let (status, started) =
        start_session(&api, &terminal, &customer.account, TEST_ATM_PIN, &new_id()).await;
    assert_eq!(status, StatusCode::OK);
    let token = started["atm_session_token"].as_str().unwrap();
    let hash = auth::token_hash(token);
    api.state
        .db
        .run(move |s| {
            assert_eq!(s.execute(&EXPIRE_SESSION, &[&hash])?, 1);
            s.commit()
        })
        .await
        .unwrap();
    for _ in 0..2 {
        let id = new_id();
        let (status, body) = finish_session(&api, token, &id).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["rsp_code"], "O0001");
        let events = rows(&api, REQUEST_EVENTS, &id).await;
        assert_eq!(
            events
                .iter()
                .filter(|e| e["event_type"] == "request.completed")
                .count(),
            1
        );
    }
    let sessions = rows(&api, SESSIONS, &terminal).await;
    assert_eq!(sessions[0]["end_reason"], "expired");
    assert_eq!(sessions[0]["ended_at"], sessions[0]["expires_at"]);
    let events = rows(
        &api,
        SESSION_EVENTS,
        sessions[0]["session_id"].as_str().unwrap(),
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"]["reason"], "expired");
}
