//! All API operations, rejection envelopes, fail-closed persistence and secrecy.
mod common;
use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const EVENTS: SqlStmt = sql_stmt!("test.outcome_events", "sql/outcome_events.sql");
const ALL: SqlStmt = sql_stmt!("test.outcome_all", "sql/outcome_all.sql");
const REFUSE: SqlStmt = sql_stmt!("test.outcome_constraint", "sql/outcome_constraint.sql");
const RESTORE: SqlStmt = sql_stmt!(
    "test.outcome_constraint_drop",
    "sql/outcome_constraint_drop.sql"
);

const RELATED: SqlStmt = sql_stmt!("test.outcome_related", "sql/outcome_related.sql");
const IDENTITY: SqlStmt = sql_stmt!("test.outcome_identity", "sql/outcome_identity.sql");
const ACCOUNT: SqlStmt = sql_stmt!("test.outcome_account", "sql/outcome_account.sql");
const ROLLBACK: SqlStmt = sql_stmt!("test.outcome_rollback", "sql/outcome_rollback.sql");

static SECRETS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
fn secret(value: &str) {
    let mut secrets = SECRETS.lock().unwrap();
    if !secrets.iter().any(|s| s == value) {
        secrets.push(value.to_owned());
    }
}
fn collect_secrets(body: &Value) {
    match body {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "password" | "new_password" | "code" | "client_secret"
                ) {
                    if let Some(value) = value.as_str() {
                        secret(value);
                    }
                } else {
                    collect_secrets(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_secrets(value);
            }
        }
        _ => {}
    }
}
async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    id: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    if let Some(token) = token {
        secret(token);
    }
    if let Some(body) = &body {
        collect_secrets(body);
    }
    let answer = common::call(router, method, path, id, token, body).await;
    if let Some(token) = answer.1["access_token"].as_str() {
        secret(token);
    }
    answer
}
async fn customer(api: &Api, opening: i64) -> Customer {
    let n = unique_digits(10);
    let phone = format!("010{n}");
    let no = format!("C{n}");
    let password = format!("outcome-password-{}", new_id());
    let account = format!("96{n}");
    let seeded = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None,
        Some(json!({"customers": [{"customer_no": no, "name": "김수취", "phone": phone, "customer_type": "INDIVIDUAL", "password": password}]}))).await;
    assert_eq!(seeded.0, StatusCode::OK);
    let seeded = call(&api.generator, "POST", "/internal/v1/seed/accounts", &new_id(), None,
        Some(json!({"accounts": [{"customer_no": no, "account_no": account, "opening_balance": opening}]}))).await;
    assert_eq!(seeded.0, StatusCode::OK);
    let fin = seeded.1["accounts"][0]["fintech_use_num"]
        .as_str()
        .unwrap()
        .to_owned();
    let login = call(
        &api.public,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({"phone": phone, "password": password, "device_id": "outcome-session-device"})),
    )
    .await;
    assert_eq!(login.0, StatusCode::OK);
    Customer {
        token: login.1["access_token"].as_str().unwrap().to_owned(),
        account,
        fin,
    }
}
async fn state_snapshot(api: &Api) -> Vec<String> {
    api.state
        .db
        .run(|s| {
            s.query_all(
                &sql_stmt!("test.outcome_state", "sql/outcome_state.sql"),
                &[],
            )
        })
        .await
        .unwrap()
}
async fn refuse(api: &Api, id: &str, all: bool) {
    let id = id.to_owned();
    api.state
        .db
        .run(move |s| {
            s.execute(
                if all {
                    &sql_stmt!("test.outcome_refuse_all", "sql/outcome_refuse_all.sql")
                } else {
                    &REFUSE
                },
                &[&id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}
async fn restore(api: &Api) {
    api.state
        .db
        .run(|s| {
            s.execute(&RESTORE, &[])?;
            Ok(())
        })
        .await
        .unwrap();
}
fn unrecorded(handle: &metrics_exporter_prometheus::PrometheusHandle) -> u64 {
    handle
        .render()
        .lines()
        .filter(|line| line.starts_with("bank_request_outcomes_unrecorded_total{"))
        .map(|line| {
            line.split_whitespace()
                .last()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .sum()
}

async fn attempts(api: &Api, id: &str) -> Vec<Value> {
    let id = id.to_owned();
    let rows: Vec<(String, Option<String>)> = api
        .state
        .db
        .run(move |s| {
            s.query_all(
                &sql_stmt!("test.outcome_attempts", "sql/outcome_attempts.sql"),
                &[&id, &id],
            )
        })
        .await
        .unwrap();
    rows.iter()
        .map(|(row, response)| {
            let mut value: Value = serde_json::from_str(row).unwrap();
            value["RESPONSE_BYTES"] = json!(response);
            value
        })
        .collect()
}
async fn balances(api: &Api) -> Vec<(i64, i64)> {
    api.state
        .db
        .run(|s| {
            s.query_all(
                &sql_stmt!("test.outcome_balances", "sql/outcome_balances.sql"),
                &[],
            )
        })
        .await
        .unwrap()
}
async fn counts(api: &Api) -> (i64, i64) {
    api.state
        .db
        .run(|s| {
            s.query_row(
                &sql_stmt!("test.outcome_counts", "sql/outcome_counts.sql"),
                &[],
            )
        })
        .await
        .unwrap()
}
async fn scope(api: &Api, token: &str, value: &str) -> String {
    let hash = bank::shared::auth::token_hash(token);
    let value = value.to_owned();
    api.state
        .db
        .run(move |s| {
            let old: String = s.query_row(
                &sql_stmt!("test.outcome_scope", "sql/outcome_scope.sql"),
                &[&hash],
            )?;
            s.execute(
                &sql_stmt!("test.outcome_set_scope", "sql/outcome_set_scope.sql"),
                &[&value, &hash],
            )?;
            s.commit()?;
            Ok(old)
        })
        .await
        .unwrap()
}
fn assert_attempt(row: &Value, kind: &str, channel: &str, code: &Value, key: Value) {
    assert_eq!(row["STATUS"], "REJECTED");
    assert_eq!(row["RSP_CODE"], *code);
    assert_eq!(row["REQUEST_TYPE"], kind);
    assert_eq!(row["CHANNEL"], channel);
    assert_eq!(row["BANK_TRAN_ID"], key);
    for name in ["IDEM_SCOPE", "IDEM_KEY", "REQUEST_HASH"] {
        assert!(row[name].is_null(), "{row}");
    }
}

async fn related(api: &Api, id: &str) -> Vec<Value> {
    let id = id.to_owned();
    let rows: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&RELATED, &[&id]))
        .await
        .unwrap();
    rows.iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

async fn assert_completion(
    api: &Api,
    id: &str,
    method: &str,
    endpoint: &str,
    status: StatusCode,
) -> Value {
    let all = related(api, id).await;
    let completions: Vec<_> = all
        .iter()
        .filter(|e| e["event_type"] == "request.completed")
        .collect();
    assert_eq!(completions.len(), 1, "{method} {endpoint}: {all:?}");
    let event = completions[0];
    assert_eq!(all.last().unwrap()["event_id"], event["event_id"]);
    for other in all
        .iter()
        .filter(|e| e["event_type"] != "request.completed")
    {
        assert!(event["event_id"].as_str().unwrap() > other["event_id"].as_str().unwrap());
    }
    assert_eq!(event["payload"]["endpoint"], endpoint);
    assert_eq!(event["payload"]["method"], method);
    assert_eq!(event["payload"]["http_status"], status.as_u16());
    assert!(event["payload"]["duration_ms"].as_u64().is_some());
    event.clone()
}

async fn events(api: &Api, id: &str) -> Vec<Value> {
    let id = id.to_owned();
    let rows: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENTS, &[&id]))
        .await
        .unwrap();
    rows.iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

struct Coverage<'a> {
    api: &'a Api,
    covered: BTreeSet<(String, String)>,
}
impl Coverage<'_> {
    async fn success(
        &mut self,
        method: &str,
        template: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        _fallback: bool,
    ) -> Value {
        let id = new_id();
        let before = self.api.state.clock.state().await.unwrap().business_time;
        let request_body = body.clone().unwrap_or(Value::Null);
        let inbound = matches!(
            template,
            "/oauth/2.0/token" | "/v2.0/transfer/deposit/fin_num"
        );
        let router = if inbound {
            &self.api.generator
        } else {
            &self.api.public
        };
        // Each successful mutation has a fail-closed witness using its valid input.
        if matches!(
            (method, template),
            (
                "POST",
                "/v2.0/ext/auth/login"
                    | "/oauth/2.0/revoke"
                    | "/v2.0/ext/auth/password_reset/request"
                    | "/v2.0/ext/auth/password_reset/confirm"
                    | "/oauth/2.0/token"
                    | "/v2.0/transfer/deposit/fin_num"
                    | "/v2.0/ext/deposits"
                    | "/v2.0/ext/deposits/{id}/close"
                    | "/v2.0/ext/loans"
                    | "/v2.0/ext/loans/{id}/repay"
            ) | ("DELETE", "/v2.0/ext/payees")
        ) {
            let before_state = state_snapshot(self.api).await;
            let before_balances = balances(self.api).await;
            let before_counts = counts(self.api).await;
            let refused = new_id();
            refuse(self.api, &refused, false).await;
            let answer = if template == "/oauth/2.0/token" {
                raw(
                    router,
                    method,
                    path,
                    &refused,
                    None,
                    format!(
                        "client_id={}&client_secret={}&scope=oob&grant_type=client_credentials",
                        request_body["client_id"].as_str().unwrap(),
                        request_body["client_secret"].as_str().unwrap()
                    ),
                    "application/x-www-form-urlencoded",
                )
                .await
            } else {
                raw(
                    router,
                    method,
                    path,
                    &refused,
                    token,
                    request_body.to_string(),
                    "application/json",
                )
                .await
            };
            restore(self.api).await;
            assert_eq!(
                answer.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "{template}: {}",
                answer.1
            );
            assert!(related(self.api, &refused).await.is_empty());
            assert_eq!(
                state_snapshot(self.api).await,
                before_state,
                "{template} business state changed"
            );
            assert_eq!(balances(self.api).await, before_balances);
            assert_eq!(counts(self.api).await, before_counts);
            if template == "/v2.0/ext/auth/login" {
                // F3: domain event failures must also map to retryable 503.
                let refused = new_id();
                refuse(self.api, &refused, true).await;
                let answer = call(router, method, path, &refused, token, body.clone()).await;
                restore(self.api).await;
                assert_eq!(answer.0, StatusCode::SERVICE_UNAVAILABLE);
                assert!(related(self.api, &refused).await.is_empty());
                assert_eq!(state_snapshot(self.api).await, before_state);
            }
        }
        let (status, response) = if template == "/oauth/2.0/token" {
            let body = body.unwrap();
            let text = format!(
                "client_id={}&client_secret={}&scope=oob&grant_type=client_credentials",
                body["client_id"].as_str().unwrap(),
                body["client_secret"].as_str().unwrap()
            );
            raw(
                router,
                method,
                path,
                &id,
                None,
                text,
                "application/x-www-form-urlencoded",
            )
            .await
        } else if inbound {
            raw(
                router,
                method,
                path,
                &id,
                token,
                request_body.to_string(),
                "application/json",
            )
            .await
        } else {
            call(router, method, path, &id, token, body).await
        };
        assert_eq!(status, StatusCode::OK, "{method} {template}: {response}");
        // T1/T3: every route has one completion, after all domain events.
        let event = assert_completion(self.api, &id, method, template, status).await;
        assert!(envelope_errors(&event).is_empty(), "{event}");
        assert_eq!(event["rsp_code"], response["rsp_code"]);
        assert_eq!(
            event["channel"],
            if inbound { "EXTERNAL" } else { "MOBILE" }
        );
        assert_eq!(
            event["listener"],
            if inbound { "generator" } else { "public" }
        );
        // T7: business time, authenticated identity and the single-account rule.
        let after = self.api.state.clock.state().await.unwrap().business_time;
        let occurred =
            chrono::DateTime::parse_from_rfc3339(event["occurred_at"].as_str().unwrap()).unwrap();
        assert!(occurred.timestamp_millis() >= before.timestamp_millis());
        assert!(occurred.timestamp_millis() <= after.timestamp_millis());
        if inbound {
            assert_eq!(event["device_id"], "outcome-sim-device");
        }
        if !inbound && let Some(token) = token {
            let hash = bank::shared::auth::token_hash(token);
            let (customer, session): (String, String) = self
                .api
                .state
                .db
                .run(move |s| s.query_row(&IDENTITY, &[&hash]))
                .await
                .unwrap();
            assert_eq!(event["customer_no"], customer);
            assert_eq!(event["session_id"], session);
        }
        let account_route = template.starts_with("/v2.0/ext/deposits")
            || template.starts_with("/v2.0/ext/loans")
            || matches!(
                template,
                "/v2.0/account/balance/fin_num"
                    | "/v2.0/account/transaction_list/fin_num"
                    | "/v2.0/transfer/withdraw/fin_num"
                    | "/v2.0/transfer/deposit/fin_num"
            );
        if account_route {
            let fin = response["fintech_use_num"]
                .as_str()
                .or_else(|| request_body["fintech_use_num"].as_str())
                .or_else(|| request_body["req_list"][0]["fintech_use_num"].as_str())
                .map(str::to_owned)
                .or_else(|| {
                    path.split('?')
                        .nth(1)
                        .and_then(|q| {
                            q.split('&')
                                .find_map(|v| v.strip_prefix("fintech_use_num="))
                        })
                        .map(str::to_owned)
                })
                .unwrap();
            let account: String = self
                .api
                .state
                .db
                .run(move |s| s.query_row(&ACCOUNT, &[&fin]))
                .await
                .unwrap();
            assert_eq!(event["account_no"], account, "{template}");
        } else {
            assert!(event["account_no"].is_null(), "{template}: {event}");
        }
        let domain = match (method, template) {
            (_, "/v2.0/ext/auth/login") => Some("login.succeeded"),
            (_, "/oauth/2.0/revoke") => Some("logout"),
            (_, "/v2.0/ext/auth/password_reset/request") => Some("password_reset.requested"),
            (_, "/v2.0/ext/auth/password_reset/confirm") => Some("password_reset.completed"),
            ("POST", "/v2.0/ext/payees") => Some("payee.registered"),
            ("DELETE", "/v2.0/ext/payees") => Some("payee.deleted"),
            _ => None,
        };
        if let Some(domain) = domain {
            assert!(
                related(self.api, &id)
                    .await
                    .iter()
                    .any(|e| e["event_type"] == domain)
            );
        }
        if matches!(
            template,
            "/v2.0/account/balance/fin_num" | "/v2.0/transfer/withdraw/fin_num"
        ) {
            assert_eq!(event["device_id"], "outcome-session-device");
            if template == "/v2.0/transfer/withdraw/fin_num" {
                assert_eq!(
                    attempts(self.api, &id).await[0]["DEVICE_ID"],
                    "outcome-session-device"
                );
            }
        }
        // T7: stored money-moving results use the fallback and retain identity/account metadata.
        let request_type = match (method, template) {
            ("POST", "/v2.0/transfer/withdraw/fin_num") => Some("TRANSFER"),
            ("POST", "/v2.0/transfer/deposit/fin_num") => Some("INBOUND"),
            ("POST", "/v2.0/ext/deposits") => Some("TERM_OPEN"),
            ("POST", "/v2.0/ext/deposits/{id}/close") => Some("TERM_CLOSE"),
            ("POST", "/v2.0/ext/loans") => Some("LOAN_TAKE"),
            ("POST", "/v2.0/ext/loans/{id}/repay") => Some("LOAN_REPAY"),
            _ => None,
        };
        if let Some(kind) = request_type {
            assert_eq!(event["payload"]["request_type"], kind);
            // T4: an unclaimed conflict leaves the original bytes and every balance intact.
            let original = attempts(self.api, &id).await;
            let before = balances(self.api).await;
            let before_count = counts(self.api).await;
            let mut changed = request_body.clone();
            let mut conflict_path = path.to_owned();
            if kind == "INBOUND" {
                changed["req_list"][0]["tran_amt"] = json!("124");
            } else if kind == "TERM_CLOSE" {
                let target = path.split('/').nth(4).unwrap().parse::<i64>().unwrap() + 1;
                conflict_path = format!("/v2.0/ext/deposits/{target}/close");
            } else {
                changed["tran_amt"] = json!("101");
            }
            let conflict_id = new_id();
            let (conflict_status, conflict_body) = raw(
                router,
                method,
                &conflict_path,
                &conflict_id,
                token,
                changed.to_string(),
                "application/json",
            )
            .await;
            assert_eq!(
                conflict_status,
                StatusCode::CONFLICT,
                "{kind}: {conflict_body}"
            );
            assert_eq!(conflict_body["rsp_code"], "A0007");
            let rows = attempts(self.api, &conflict_id).await;
            assert_eq!(rows.len(), 1);
            let key = if inbound {
                &request_body["req_list"][0]["bank_tran_id"]
            } else {
                &request_body["bank_tran_id"]
            };
            assert_attempt(
                &rows[0],
                kind,
                if inbound { "EXTERNAL" } else { "MOBILE" },
                &json!("A0007"),
                key.clone(),
            );
            assert_eq!(counts(self.api).await, (before_count.0 + 1, before_count.1));
            assert_eq!(attempts(self.api, &id).await, original);
            assert_eq!(balances(self.api).await, before);
            let conflict_event =
                assert_completion(self.api, &conflict_id, method, template, conflict_status).await;
            assert_eq!(conflict_event["payload"]["request_type"], kind);
            // T4: both authorization failure paths persist exactly one unclaimed attempt.
            for forbidden in [false, true] {
                let before_balances = balances(self.api).await;
                let before_counts = counts(self.api).await;
                let mut auth_body = request_body.clone();
                if inbound {
                    auth_body["req_list"][0]["bank_tran_id"] = json!(bank_tran_id());
                } else {
                    auth_body["bank_tran_id"] = json!(bank_tran_id());
                }
                let auth_id = new_id();
                let old_scope = if forbidden {
                    Some(scope(self.api, token.unwrap(), "inquiry").await)
                } else {
                    None
                };
                let answer = raw(
                    router,
                    method,
                    path,
                    &auth_id,
                    if forbidden { token } else { None },
                    auth_body.to_string(),
                    "application/json",
                )
                .await;
                if let Some(old) = old_scope {
                    scope(self.api, token.unwrap(), &old).await;
                }
                assert_eq!(
                    answer.0,
                    if forbidden {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::UNAUTHORIZED
                    },
                    "{kind}: {}",
                    answer.1
                );
                assert_eq!(balances(self.api).await, before_balances);
                assert_eq!(counts(self.api).await.1, before_counts.1);
                let rows = attempts(self.api, &auth_id).await;
                assert_eq!(rows.len(), 1);
                assert_attempt(
                    &rows[0],
                    kind,
                    if inbound { "EXTERNAL" } else { "MOBILE" },
                    &answer.1["rsp_code"],
                    Value::Null,
                );
                let event = assert_completion(self.api, &auth_id, method, template, answer.0).await;
                assert_eq!(event["payload"]["request_type"], kind);
            }
            let malformed = new_id();
            let answer = raw(
                router,
                method,
                path,
                &malformed,
                token,
                "{".into(),
                "application/json",
            )
            .await;
            assert_eq!(answer.0, StatusCode::BAD_REQUEST);
            assert!(attempts(self.api, &malformed).await.is_empty());
            let replay_id = new_id();
            let (replay_status, replay_body) = if inbound {
                raw(
                    router,
                    method,
                    path,
                    &replay_id,
                    token,
                    request_body.to_string(),
                    "application/json",
                )
                .await
            } else {
                call(
                    router,
                    method,
                    path,
                    &replay_id,
                    token,
                    Some(request_body.clone()),
                )
                .await
            };
            assert_eq!(replay_status, status);
            assert_eq!(
                common::deposits::core(replay_body),
                common::deposits::core(response.clone())
            );
            assert_eq!(attempts(self.api, &id).await, original);
            let replay =
                assert_completion(self.api, &replay_id, method, template, replay_status).await;
            assert_eq!(related(self.api, &replay_id).await.len(), 1);
            assert_eq!(replay["payload"]["replayed"], true);
            assert_eq!(replay["payload"]["request_type"], kind);
            for key in ["account_no", "customer_no", "session_id", "device_id"] {
                assert_eq!(replay[key], event[key], "replay {template} {key}");
            }
        }
        self.covered
            .insert((template.to_owned(), method.to_owned()));
        response
    }
}
async fn raw(
    router: &axum::Router,
    method: &str,
    path: &str,
    id: &str,
    token: Option<&str>,
    body: String,
    content_type: &str,
) -> (StatusCode, Value) {
    if let Some(token) = token {
        secret(token);
    }
    if let Ok(value) = serde_json::from_str::<Value>(&body) {
        collect_secrets(&value);
    }
    if content_type == "application/x-www-form-urlencoded" {
        for part in body.split('&') {
            if let Some(value) = part.strip_prefix("client_secret=") {
                secret(value);
            }
        }
    }
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-request-id", id)
        .header("content-type", content_type)
        .header("x-sim-device", "outcome-sim-device");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn contract_operations() -> BTreeSet<(String, String)> {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".into());
    let mut operations = BTreeSet::new();
    for name in ["public", "inbound"] {
        let text = std::fs::read_to_string(format!("{dir}/openapi/{name}.yaml")).unwrap();
        let mut path = None;
        for line in text.lines() {
            if line.starts_with("  /") && line.ends_with(':') {
                path = Some(line.trim().trim_end_matches(':').to_owned());
            } else if let Some(method) = line.strip_prefix("    ").and_then(|v| v.strip_suffix(':'))
                && matches!(
                    method,
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
                )
            {
                operations.insert((
                    path.clone().expect("path before method"),
                    method.to_uppercase(),
                ));
            }
        }
    }
    operations
}

// One test serializes constraint DDL with all requests in this file.
#[tokio::test]
async fn outcomes() {
    let metrics = bank::shared::telemetry::metrics_handle();
    let api = api().await;
    let a = customer(&api, 5_000_000).await;
    let b = customer(&api, 0).await;
    let mut coverage = Coverage {
        api: &api,
        covered: BTreeSet::new(),
    };
    let n = unique_digits(10);
    let phone = format!("010{n}");
    let password = "outcome-password-unique";
    let (status, _) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None, Some(json!({"customers": [{"customer_no": format!("O{n}"), "name": "결과", "phone": phone, "customer_type": "INDIVIDUAL", "password": password}]}))).await;
    assert_eq!(status, StatusCode::OK);
    // T3: rejected login still commits its completion after login.failed.
    let wrong_id = new_id();
    let (status, _) = call(
        &api.public,
        "POST",
        "/v2.0/ext/auth/login",
        &wrong_id,
        None,
        Some(json!({"phone": phone, "password": "outcome-wrong-password-sentinel"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let wrong = assert_completion(&api, &wrong_id, "POST", "/v2.0/ext/auth/login", status).await;
    assert_eq!(wrong["outcome"], "rejected");
    assert!(
        related(&api, &wrong_id)
            .await
            .iter()
            .any(|e| e["event_type"] == "login.failed")
    );
    let login = coverage
        .success(
            "POST",
            "/v2.0/ext/auth/login",
            "/v2.0/ext/auth/login",
            None,
            Some(json!({"phone": phone, "password": password, "device_id": "outcome-device"})),
            true,
        )
        .await;
    let login_token = login["access_token"].as_str().unwrap();
    coverage
        .success(
            "POST",
            "/oauth/2.0/revoke",
            "/oauth/2.0/revoke",
            Some(login_token),
            None,
            true,
        )
        .await;
    for path in [
        "/v2.0/user/me",
        "/v2.0/account/list",
        "/v2.0/ext/products",
        "/v2.0/ext/payees",
    ] {
        coverage
            .success("GET", path, path, Some(&a.token), None, true)
            .await;
    }
    let time = api.state.clock.state().await.unwrap().business_time;
    let dt = time.format("%Y%m%d%H%M%S").to_string();
    let balance = format!(
        "/v2.0/account/balance/fin_num?bank_tran_id={}&fintech_use_num={}&tran_dtime={dt}",
        bank_tran_id(),
        a.fin
    );
    coverage
        .success(
            "GET",
            "/v2.0/account/balance/fin_num",
            &balance,
            Some(&a.token),
            None,
            true,
        )
        .await;
    let history = format!(
        "/v2.0/account/transaction_list/fin_num?bank_tran_id={}&fintech_use_num={}&tran_dtime={dt}&inquiry_type=A&inquiry_base=D&from_date={}&to_date={}&sort_order=D",
        bank_tran_id(),
        a.fin,
        time.format("%Y%m%d"),
        time.format("%Y%m%d")
    );
    coverage
        .success(
            "GET",
            "/v2.0/account/transaction_list/fin_num",
            &history,
            Some(&a.token),
            None,
            true,
        )
        .await;
    coverage
        .success(
            "POST",
            "/v2.0/inquiry/receive",
            "/v2.0/inquiry/receive",
            Some(&a.token),
            Some(json!({"bank_tran_id": bank_tran_id(), "bank_code_std": "998", "account_num": b.account, "tran_amt": "100", "tran_dtime": dt})),
            true,
        )
        .await;
    let payee = coverage
        .success(
            "POST",
            "/v2.0/ext/payees",
            "/v2.0/ext/payees",
            Some(&a.token),
            Some(json!({"bank_code_std": "998", "account_num": b.account, "alias": "결과"})),
            true,
        )
        .await;
    coverage
        .success(
            "DELETE",
            "/v2.0/ext/payees",
            &format!(
                "/v2.0/ext/payees?payee_id={}",
                payee["payee_id"].as_str().unwrap()
            ),
            Some(&a.token),
            None,
            true,
        )
        .await;
    let btid = bank_tran_id();
    coverage.success("POST", "/v2.0/transfer/withdraw/fin_num", "/v2.0/transfer/withdraw/fin_num", Some(&a.token), Some(json!({"bank_tran_id": btid, "fintech_use_num": a.fin, "tran_amt": "100", "tran_dtime": dt, "transfer_purpose": "TR", "recv_client_name": "김수취", "recv_client_bank_code": "998", "recv_client_account_num": b.account})), false).await;
    coverage.success("POST", "/v2.0/transfer/result", "/v2.0/transfer/result", Some(&a.token), Some(json!({"check_type": "1", "tran_dtime": dt, "req_cnt": "1", "req_list": [{"tran_no": "1", "org_bank_tran_id": btid, "org_bank_tran_date": time.format("%Y%m%d").to_string(), "org_tran_amt": "100"}]})), true).await;
    let deposit = coverage
        .success(
            "POST",
            "/v2.0/ext/deposits",
            "/v2.0/ext/deposits",
            Some(&a.token),
            Some(common::deposits::open_payload(&api, &a.fin, 100_000).await),
            false,
        )
        .await;
    let id = deposit["deposit_id"].as_str().unwrap();
    coverage
        .success(
            "GET",
            "/v2.0/ext/deposits/{id}",
            &format!("/v2.0/ext/deposits/{id}"),
            Some(&a.token),
            None,
            true,
        )
        .await;
    coverage
        .success(
            "POST",
            "/v2.0/ext/deposits/{id}/close",
            &format!("/v2.0/ext/deposits/{id}/close"),
            Some(&a.token),
            Some(common::deposits::close_payload(&api).await),
            false,
        )
        .await;
    let loan = coverage.success("POST", "/v2.0/ext/loans", "/v2.0/ext/loans", Some(&a.token), Some(json!({"bank_tran_id": bank_tran_id(), "fintech_use_num": a.fin, "terms_id": "5", "tran_amt": "1000000", "tran_dtime": dt})), false).await;
    let id = loan["loan_id"].as_str().unwrap();
    coverage
        .success(
            "GET",
            "/v2.0/ext/loans/{id}",
            &format!("/v2.0/ext/loans/{id}"),
            Some(&a.token),
            None,
            true,
        )
        .await;
    coverage
        .success(
            "POST",
            "/v2.0/ext/loans/{id}/repay",
            &format!("/v2.0/ext/loans/{id}/repay"),
            Some(&a.token),
            Some(json!({"bank_tran_id": bank_tran_id(), "tran_amt": "1000000", "tran_dtime": dt})),
            false,
        )
        .await;
    coverage
        .success(
            "POST",
            "/v2.0/ext/auth/password_reset/request",
            "/v2.0/ext/auth/password_reset/request",
            None,
            Some(json!({"phone": phone})),
            true,
        )
        .await;
    let (_, sms) = call(
        &api.generator,
        "GET",
        &format!("/internal/v1/sms/{phone}"),
        &new_id(),
        None,
        None,
    )
    .await;
    let code = sms["code"].as_str().unwrap();
    coverage
        .success(
            "POST",
            "/v2.0/ext/auth/password_reset/confirm",
            "/v2.0/ext/auth/password_reset/confirm",
            None,
            Some(json!({"phone": phone, "code": code, "new_password": "outcome-new-password"})),
            true,
        )
        .await;
    let client = format!("outcome_{}", new_id());
    let secret = "outcome-client-secret";
    let (status, _) = call(&api.generator, "POST", "/internal/v1/seed/counterparty_clients", &new_id(), None, Some(json!({"counterparty_clients": [{"client_id": client, "client_secret": secret, "institution_code": "901", "scope": "oob"}]}))).await;
    assert_eq!(status, StatusCode::OK);
    let issued = coverage
        .success(
            "POST",
            "/oauth/2.0/token",
            "/oauth/2.0/token",
            None,
            Some(json!({"client_id": client, "client_secret": secret})),
            true,
        )
        .await;
    let client_token = issued["access_token"].as_str().unwrap();
    let sender = format!("94{}", unique_digits(10));
    seed_counterparty(&api, &sender).await;
    coverage.success("POST", "/v2.0/transfer/deposit/fin_num", "/v2.0/transfer/deposit/fin_num", Some(client_token), Some(json!({"cntr_account_type": "N", "cntr_account_num": sender, "wd_print_content": "송금", "name_check_option": "off", "tran_dtime": dt, "req_cnt": "1", "req_list": [{"tran_no": "1", "bank_tran_id": bank_tran_id(), "fintech_use_num": b.fin, "print_content": "입금", "tran_amt": "123", "req_client_name": "외부수취인", "transfer_purpose": "TR"}]})), false).await;
    // T4: request-ID reuse never changes the original movement or replay result.
    let path = "/v2.0/transfer/withdraw/fin_num";
    let transfer = json!({"bank_tran_id": bank_tran_id(), "fintech_use_num": a.fin, "tran_amt": "10",
        "tran_dtime": dt, "transfer_purpose": "TR", "recv_client_name": "김수취",
        "recv_client_bank_code": "998", "recv_client_account_num": b.account});
    let reused_id = new_id();
    let first = call(
        &api.public,
        "POST",
        path,
        &reused_id,
        Some(&a.token),
        Some(transfer.clone()),
    )
    .await;
    assert_eq!(first.0, StatusCode::OK);
    let original = attempts(&api, &reused_id).await;
    let before = balances(&api).await;
    let before_count = counts(&api).await;
    let mut second = transfer.clone();
    second["bank_tran_id"] = json!(bank_tran_id());
    second["tran_amt"] = json!("11");
    let answer = call(
        &api.public,
        "POST",
        path,
        &reused_id,
        Some(&a.token),
        Some(second),
    )
    .await;
    assert_eq!(answer.0, StatusCode::CONFLICT);
    assert_eq!(answer.1["rsp_code"], "A0007");
    let rows = attempts(&api, &reused_id).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&original[0]));
    let rejected = rows
        .iter()
        .find(|row| row["REUSED_REQUEST_ID"] == reused_id)
        .unwrap();
    assert_ne!(rejected["REQUEST_ID"], reused_id);
    let completion: Vec<_> = related(&api, &reused_id)
        .await
        .into_iter()
        .filter(|e| e["event_type"] == "request.completed")
        .collect();
    assert_eq!(completion.len(), 2);
    assert_eq!(
        completion[1]["payload"]["attempt_id"],
        rejected["REQUEST_ID"]
    );
    assert_eq!(completion[1]["payload"]["http_status"], 409);
    assert_eq!(balances(&api).await, before);
    assert_eq!(counts(&api).await, (before_count.0 + 1, before_count.1));
    let replay = call(
        &api.public,
        "POST",
        path,
        &reused_id,
        Some(&a.token),
        Some(transfer.clone()),
    )
    .await;
    assert_eq!(replay.0, first.0);
    assert_eq!(
        common::deposits::core(replay.1),
        common::deposits::core(first.1)
    );
    assert_eq!(attempts(&api, &reused_id).await, rows);
    let replay_events = events(&api, &reused_id).await;
    assert_eq!(replay_events.len(), completion.len() + 1);
    assert_eq!(replay_events.last().unwrap()["payload"]["replayed"], true);

    // F1: another claimed request ID cannot replay an unrelated key.
    let owner_id = new_id();
    let mut owner_transfer = transfer.clone();
    owner_transfer["bank_tran_id"] = json!(bank_tran_id());
    assert_eq!(
        call(
            &api.public,
            "POST",
            path,
            &owner_id,
            Some(&a.token),
            Some(owner_transfer)
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut other_transfer = transfer.clone();
    other_transfer["bank_tran_id"] = json!(bank_tran_id());
    let other_id = new_id();
    assert_eq!(
        call(
            &api.public,
            "POST",
            path,
            &other_id,
            Some(&a.token),
            Some(other_transfer.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    let other_original = attempts(&api, &other_id).await;
    let original_rows = attempts(&api, &owner_id).await;
    let before = balances(&api).await;
    let before_count = counts(&api).await;
    let answer = call(
        &api.public,
        "POST",
        path,
        &owner_id,
        Some(&a.token),
        Some(other_transfer),
    )
    .await;
    assert_eq!(answer.0, StatusCode::CONFLICT);
    assert_eq!(answer.1["rsp_code"], "A0007");
    let after_rows = attempts(&api, &owner_id).await;
    assert_eq!(after_rows.len(), original_rows.len() + 1);
    assert!(original_rows.iter().all(|row| after_rows.contains(row)));
    assert_eq!(attempts(&api, &other_id).await, other_original);
    assert_eq!(balances(&api).await, before);
    assert_eq!(counts(&api).await, (before_count.0 + 1, before_count.1));
    assert_eq!(
        after_rows
            .iter()
            .filter(|row| row["REUSED_REQUEST_ID"] == owner_id && row["STATUS"] == "REJECTED")
            .count(),
        1
    );

    let rejected = after_rows
        .iter()
        .find(|row| row["REUSED_REQUEST_ID"] == owner_id)
        .unwrap();
    assert_ne!(rejected["REQUEST_ID"], owner_id);
    let completed = events(&api, &owner_id).await;
    assert_eq!(completed.len(), 2);
    assert_eq!(
        completed[1]["payload"]["attempt_id"],
        rejected["REQUEST_ID"]
    );
    assert_eq!(completed[1]["payload"]["http_status"], 409);

    // F2: unclaimed auth rejections are relocated, then the original ID can claim.
    let mut retry_transfer = transfer.clone();
    retry_transfer["bank_tran_id"] = json!(bank_tran_id());
    let retry_inbound = json!({"cntr_account_type": "N", "cntr_account_num": sender,
        "wd_print_content": "송금", "name_check_option": "off", "tran_dtime": dt,
        "req_cnt": "1", "req_list": [{"tran_no": "1", "bank_tran_id": bank_tran_id(),
        "fintech_use_num": b.fin, "print_content": "입금", "tran_amt": "123",
        "req_client_name": "외부수취인", "transfer_purpose": "TR"}]});
    for (router, retry_path, token, body, amount) in [
        (&api.public, path, a.token.as_str(), retry_transfer, 10),
        (
            &api.generator,
            "/v2.0/transfer/deposit/fin_num",
            client_token,
            retry_inbound,
            123,
        ),
    ] {
        let id = new_id();
        let before = common::deposits::snapshot(&api, &b.fin).await;
        let before_counts = counts(&api).await;
        let rejected = call(router, "POST", retry_path, &id, None, Some(body.clone())).await;
        assert_eq!(rejected.0, StatusCode::UNAUTHORIZED);
        let original = attempts(&api, &id).await;
        assert_eq!(original.len(), 1);
        assert!(original[0]["IDEM_KEY"].is_null());
        let accepted = call(router, "POST", retry_path, &id, Some(token), Some(body)).await;
        assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
        let rows = attempts(&api, &id).await;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .filter(|r| r["REQUEST_ID"] == id && r["IDEM_KEY"].is_string())
                .count(),
            1
        );
        let moved = rows.iter().find(|r| r["REUSED_REQUEST_ID"] == id).unwrap();
        assert_ne!(moved["REQUEST_ID"], id);
        assert_eq!(moved["STATUS"], "REJECTED");
        assert!(moved["IDEM_KEY"].is_null());
        let mut expected_rejection = original[0].clone();
        expected_rejection["REQUEST_ID"] = moved["REQUEST_ID"].clone();
        expected_rejection["REUSED_REQUEST_ID"] = json!(id);
        assert_eq!(*moved, expected_rejection);
        let completed = events(&api, &id).await;
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0]["payload"]["http_status"], 401);
        assert_eq!(completed[1]["payload"]["http_status"], 200);
        assert_eq!(
            common::deposits::snapshot(&api, &b.fin).await.0,
            before.0 + amount
        );
        assert_eq!(
            counts(&api).await,
            (before_counts.0 + 2, before_counts.1 + 1)
        );
    }

    // Eight simultaneous duplicates each complete, while only one claims the key and posts.
    let mut concurrent = transfer;
    concurrent["bank_tran_id"] = json!(bank_tran_id());
    let before_a = common::deposits::snapshot(&api, &a.fin).await;
    let before_b = common::deposits::snapshot(&api, &b.fin).await;
    let before_count = counts(&api).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let router = api.public.clone();
        let token = a.token.clone();
        let body = concurrent.clone();
        tasks.spawn(async move {
            let id = new_id();
            let answer = call(&router, "POST", path, &id, Some(&token), Some(body)).await;
            (id, answer)
        });
    }
    let mut answers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (id, answer) = result.unwrap();
        assert_eq!(answer.0, StatusCode::OK);
        assert_completion(&api, &id, "POST", path, answer.0).await;
        answers.push(common::deposits::core(answer.1));
    }
    assert_eq!(answers.len(), 8);
    assert!(answers.iter().all(|answer| answer == &answers[0]));
    assert_eq!(counts(&api).await, (before_count.0 + 1, before_count.1 + 1));
    assert_eq!(
        common::deposits::snapshot(&api, &a.fin).await.0,
        before_a.0 - 10
    );
    assert_eq!(
        common::deposits::snapshot(&api, &b.fin).await.0,
        before_b.0 + 10
    );

    // Unknown inbound senders retain a replayable, claimed rejection.
    let unknown = json!({"cntr_account_type": "N", "cntr_account_num": "9999999999999999",
        "wd_print_content": "송금", "name_check_option": "off", "tran_dtime": dt,
        "req_cnt": "1", "req_list": [{"tran_no": "1", "bank_tran_id": bank_tran_id(),
        "fintech_use_num": b.fin, "print_content": "입금", "tran_amt": "123",
        "req_client_name": "외부수취인", "transfer_purpose": "TR"}]});
    let path = "/v2.0/transfer/deposit/fin_num";
    let id = new_id();
    let before_count = counts(&api).await;
    let before = balances(&api).await;
    let answer = call(
        &api.generator,
        "POST",
        path,
        &id,
        Some(client_token),
        Some(unknown.clone()),
    )
    .await;
    assert_eq!(answer.0, StatusCode::BAD_REQUEST);
    assert_eq!(answer.1["rsp_code"], "A0002");
    let rows = attempts(&api, &id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["STATUS"], "REJECTED");
    assert_eq!(rows[0]["RSP_CODE"], "A0002");
    for key in ["IDEM_SCOPE", "IDEM_KEY", "REQUEST_HASH", "RESPONSE_BYTES"] {
        assert!(rows[0][key].is_string());
    }
    assert_completion(&api, &id, "POST", path, answer.0).await;
    let replay = call(
        &api.generator,
        "POST",
        path,
        &new_id(),
        Some(client_token),
        Some(unknown),
    )
    .await;
    assert_eq!(replay.0, answer.0);
    assert_eq!(
        common::deposits::core(replay.1),
        common::deposits::core(answer.1)
    );
    assert_eq!(counts(&api).await, (before_count.0 + 1, before_count.1));
    assert_eq!(balances(&api).await, before);
    assert_eq!(attempts(&api, &id).await, rows);

    assert_eq!(
        coverage.covered,
        contract_operations(),
        "every OpenAPI operation has a successful witness"
    );

    let expired_token = format!("outcome-expired-token-{}", new_id());
    let hash = bank::shared::auth::token_hash(&expired_token);
    let source = bank::shared::auth::token_hash(&a.token);
    api.state
        .db
        .run(move |s| {
            s.execute(
                &sql_stmt!("test.outcome_expired", "sql/outcome_expired.sql"),
                &[&hash, &source],
            )?;
            s.commit()
        })
        .await
        .unwrap();
    // T2: framework and extractor rejections all pass through the same fallback.
    for (method, path, token, text, expected, endpoint) in [
        (
            "GET",
            "/v2.0/user/me",
            None,
            String::new(),
            401,
            "/v2.0/user/me",
        ),
        (
            "GET",
            "/v2.0/user/me",
            Some("outcome-unknown-token-sentinel"),
            String::new(),
            401,
            "/v2.0/user/me",
        ),
        (
            "GET",
            "/v2.0/user/me",
            Some(expired_token.as_str()),
            String::new(),
            401,
            "/v2.0/user/me",
        ),
        (
            "POST",
            "/v2.0/ext/auth/login",
            None,
            "{".into(),
            400,
            "/v2.0/ext/auth/login",
        ),
        (
            "GET",
            "/v2.0/account/balance/fin_num?fintech_use_num=%FF",
            Some(a.token.as_str()),
            String::new(),
            400,
            "/v2.0/account/balance/fin_num",
        ),
        (
            "POST",
            "/v2.0/ext/auth/login",
            None,
            "x".repeat(1_048_577),
            413,
            "/v2.0/ext/auth/login",
        ),
        (
            "GET",
            "/not-an-api?secret=never-store",
            None,
            String::new(),
            404,
            "unmatched",
        ),
        (
            "POST",
            "/v2.0/user/me",
            None,
            String::new(),
            405,
            "/v2.0/user/me",
        ),
    ] {
        let id = new_id();
        let (status, body) = raw(
            &api.public,
            method,
            path,
            &id,
            token,
            text,
            "application/json",
        )
        .await;
        assert_eq!(status.as_u16(), expected, "{body}");
        if matches!(expected, 404 | 405) {
            assert_contract("Error", &body);
            assert_eq!(
                body["rsp_code"],
                if expected == 404 { "A0003" } else { "A0002" }
            );
        }
        if expected == 400 {
            assert_contract("Error", &body);
            assert_eq!(body["rsp_code"], "A0002");
        }
        let found = events(&api, &id).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["outcome"], "rejected");
        assert_eq!(found[0]["payload"]["http_status"], expected);
        assert_eq!(found[0]["payload"]["endpoint"], endpoint);
        assert_eq!(found[0]["rsp_code"], body["rsp_code"]);
    }
    let options_id = new_id();
    call(
        &api.generator,
        "OPTIONS",
        "/v2.0/user/me",
        &options_id,
        None,
        None,
    )
    .await;
    assert!(related(&api, &options_id).await.is_empty());
    for path in ["/healthz", "/internal/v1/clock", "/internal/unknown"] {
        let id = new_id();
        call(&api.generator, "GET", path, &id, None, None).await;
        assert!(events(&api, &id).await.is_empty());
    }
    // T3: refuse only completion INSERTs, then prove the entire transaction rolled back.
    // The constraint is always restored before any assertions or reads that may panic.
    let rollback_payee = customer(&api, 0).await;
    for (path, token, body) in [
        (
            "/v2.0/ext/auth/login",
            None,
            json!({"phone": phone, "password": "outcome-new-password"}),
        ),
        (
            "/v2.0/ext/payees",
            Some(a.token.as_str()),
            json!({"bank_code_std": "998", "account_num": rollback_payee.account}),
        ),
        (
            "/v2.0/transfer/withdraw/fin_num",
            Some(a.token.as_str()),
            json!({"bank_tran_id": bank_tran_id(), "fintech_use_num": a.fin, "tran_amt": "100", "tran_dtime": dt, "transfer_purpose": "TR", "recv_client_name": "김수취", "recv_client_bank_code": "998", "recv_client_account_num": rollback_payee.account}),
        ),
    ] {
        let before_a = common::deposits::snapshot(&api, &a.fin).await;
        let before_b = common::deposits::snapshot(&api, &rollback_payee.fin).await;
        let refused = new_id();
        let id = refused.clone();
        api.state
            .db
            .run(move |s| {
                s.execute(&REFUSE, &[&id])?;
                Ok(())
            })
            .await
            .unwrap();
        let answer = call(&api.public, "POST", path, &refused, token, Some(body)).await;
        api.state
            .db
            .run(|s| {
                s.execute(&RESTORE, &[])?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(answer.0, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert!(related(&api, &refused).await.is_empty());
        let account = rollback_payee.account.clone();
        let counts: (i64, i64, i64) = api
            .state
            .db
            .run(move |s| s.query_row(&ROLLBACK, &[&refused, &refused, &account]))
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0), "{path}");
        assert_eq!(common::deposits::snapshot(&api, &a.fin).await, before_a);
        assert_eq!(
            common::deposits::snapshot(&api, &rollback_payee.fin).await,
            before_b
        );
    }

    // T5: always drop the constraint before propagating assertion failures.
    let refused = new_id();
    let id = refused.clone();
    api.state
        .db
        .run(move |s| {
            s.execute(&REFUSE, &[&id])?;
            Ok(())
        })
        .await
        .unwrap();
    let counter_before = unrecorded(&metrics);
    let blocked = call(&api.public, "GET", &balance, &refused, Some(&a.token), None).await;
    let counter_after = unrecorded(&metrics);
    let blocked_events = events(&api, &refused).await;
    let other_id = new_id();
    let other = call(
        &api.public,
        "GET",
        &balance,
        &other_id,
        Some(&a.token),
        None,
    )
    .await;
    let other_events = events(&api, &other_id).await;
    api.state
        .db
        .run(|s| {
            s.execute(&RESTORE, &[])?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(counter_after, counter_before + 1);
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(blocked.1["rsp_code"], "A0013");
    assert!(blocked_events.is_empty());
    assert_eq!(other.0, StatusCode::OK);
    assert_eq!(other_events.len(), 1);

    // T6: inspect the complete OUTBOX, including handler-owned events.
    let rows: Vec<String> = api.state.db.run(|s| s.query_all(&ALL, &[])).await.unwrap();
    for text in rows {
        for secret in SECRETS.lock().unwrap().iter() {
            assert!(!text.contains(secret), "secret leaked into OUTBOX");
        }
        let event: Value = serde_json::from_str(&text).unwrap();
        if event["event_type"] == "request.completed" {
            for key in event["payload"].as_object().unwrap().keys() {
                assert!(
                    [
                        "endpoint",
                        "method",
                        "http_status",
                        "duration_ms",
                        "request_type",
                        "replayed",
                        // Fields the money-moving handlers already publish.
                        "tran_amt",
                        "transfer_id",
                        "reason",
                        "counterparty_bank_code",
                        "counterparty_account_no",
                        "attempt_id"
                    ]
                    .contains(&key.as_str()),
                    "unexpected completion key {key}"
                );
            }
        }
    }
}
