//! Helpers shared by the integration tests.
#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bank::app::{self, AppState};
use bank::shared::clock::Clock;
use bank::shared::db::{Db, DbConfig};
use bank::shared::rollover::Registry;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub async fn state() -> AppState {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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

pub struct Api {
    pub public: Router,
    pub generator: Router,
    pub state: AppState,
}

pub async fn api() -> Api {
    let state = state().await;
    Api {
        public: app::public_router(state.clone()),
        generator: app::generator_router(state.clone()),
        state,
    }
}

pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

/// A short unique numeric suffix for test data.
pub fn unique_digits(n: usize) -> String {
    let id = ulid::Ulid::new();
    let digits: String = format!("{}", id.0).chars().rev().take(n).collect();
    digits
}

pub async fn call(
    router: &Router,
    method: &str,
    path: &str,
    request_id: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-request-id", request_id);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let res = router.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Validates a JSON value against a schema in `contracts/openapi/public.yaml`.
/// Returns the validation errors, empty when valid.
pub fn contract_errors(schema_name: &str, value: &Value) -> Vec<String> {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".to_owned());
    let text = std::fs::read_to_string(format!("{dir}/openapi/public.yaml"))
        .expect("read the public contract");
    let mut doc: Value = serde_yaml::from_str(&text).expect("contract YAML");
    if doc["components"]["schemas"].get(schema_name).is_none() {
        let text = std::fs::read_to_string(format!("{dir}/openapi/internal.yaml"))
            .expect("internal contract");
        doc = serde_yaml::from_str(&text).expect("internal YAML");
    }
    let defs = doc["components"]["schemas"]
        .to_string()
        .replace("#/components/schemas/", "#/$defs/");
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": serde_json::from_str::<Value>(&defs).expect("defs"),
        "$ref": format!("#/$defs/{schema_name}"),
    });
    let validator = jsonschema::draft202012::new(&schema).expect("valid contract schema");
    validator
        .iter_errors(value)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

pub fn assert_contract(schema_name: &str, value: &Value) {
    let errors = contract_errors(schema_name, value);
    assert!(
        errors.is_empty(),
        "{schema_name} contract violations: {errors:?}\n{value}"
    );
}

/// A valid `bank_tran_id` for tests.
pub fn bank_tran_id() -> String {
    format!("T{}U{}", unique_digits(9), unique_digits(9))
}

/// Validates a business event against the shared log envelope schema.
pub fn envelope_errors(event: &Value) -> Vec<String> {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".to_owned());
    let text = std::fs::read_to_string(format!("{dir}/schemas/log-envelope.schema.json"))
        .expect("read the log envelope contract");
    let schema: Value = serde_json::from_str(&text).expect("envelope JSON");
    let validator = jsonschema::draft202012::new(&schema).expect("valid envelope schema");
    validator
        .iter_errors(event)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

pub struct Customer {
    pub token: String,
    pub account: String,
    pub fin: String,
}
pub async fn customer(api: &Api, opening: i64) -> Customer {
    let n = unique_digits(10);
    let phone = format!("010{n}");
    let no = format!("C{n}");
    let account = format!("96{n}");
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(
            serde_json::json!({"customers": [{"customer_no": no, "name": "김수취", "phone": phone,
        "customer_type": "INDIVIDUAL", "password": "pw-payees"}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("CreatedResponse", &body);
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/accounts", &new_id(), None,
        Some(serde_json::json!({"accounts": [{"customer_no": no, "account_no": account, "opening_balance": opening}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("SeedAccountsResponse", &body);
    let fin = body["accounts"][0]["fintech_use_num"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = call(
        &api.public,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(
            serde_json::json!({"phone": phone, "password": "pw-payees", "device_id": "payee-test"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("LoginResponse", &body);
    Customer {
        token: body["access_token"].as_str().unwrap().to_owned(),
        account,
        fin,
    }
}
pub async fn seed_counterparty(api: &Api, account: &str) {
    let request = serde_json::json!({"counterparties": [{"counterparty_type": "INDIVIDUAL", "institution_code": "901",
        "account_no": account, "name": "외부수취인"}]});
    for expected in [1, 0] {
        let (status, body) = call(
            &api.generator,
            "POST",
            "/internal/v1/seed/counterparties",
            &new_id(),
            None,
            Some(request.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_contract("CreatedResponse", &body);
        assert_eq!(body["created"], expected);
    }
}
pub async fn register_payee(api: &Api, token: &str, bank: &str, account: &str) -> Value {
    let (status, body) = call(
        &api.public,
        "POST",
        "/v2.0/ext/payees",
        &new_id(),
        Some(token),
        Some(serde_json::json!({"bank_code_std": bank, "account_num": account, "alias": "수취인 별칭"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("PayeeResponse", &body);
    body
}

/// Validate the separate generator inbound contract rather than public.yaml.
pub fn assert_inbound_contract(schema_name: &str, value: &Value) {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".to_owned());
    let text =
        std::fs::read_to_string(format!("{dir}/openapi/inbound.yaml")).expect("inbound contract");
    let doc: Value = serde_yaml::from_str(&text).expect("inbound YAML");
    let defs = doc["components"]["schemas"]
        .to_string()
        .replace("#/components/schemas/", "#/$defs/");
    let schema = serde_json::json!({"$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": serde_json::from_str::<Value>(&defs).expect("defs"), "$ref": format!("#/$defs/{schema_name}")});
    let validator = jsonschema::draft202012::new(&schema).expect("inbound schema");
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{schema_name}: {errors:?}\n{value}");
}

pub async fn form_call(router: &Router, path: &str, body: String) -> (StatusCode, Value) {
    let req = Request::post(path)
        .header("x-request-id", new_id())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("request");
    let res = router.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

pub mod deposits;
