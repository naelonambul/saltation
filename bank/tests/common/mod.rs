//! Helpers shared by the integration tests.
#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bank::app::{self, AppState};
use bank::shared::clock::Clock;
use bank::shared::db::{Db, DbConfig};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub async fn state() -> AppState {
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");
    db.migrate().await.expect("migrate");
    let clock = Clock::init(db.clone()).await.expect("clock");
    AppState { db, clock }
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
    let doc: Value = serde_yaml::from_str(&text).expect("contract YAML");
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
