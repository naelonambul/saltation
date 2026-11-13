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
        fallback: bool,
    ) -> Value {
        let id = new_id();
        let inbound = matches!(
            template,
            "/oauth/2.0/token" | "/v2.0/transfer/deposit/fin_num"
        );
        let router = if inbound {
            &self.api.generator
        } else {
            &self.api.public
        };
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
        } else {
            call(router, method, path, &id, token, body).await
        };
        assert_eq!(status, StatusCode::OK, "{method} {template}: {response}");
        let found = events(self.api, &id).await;
        assert_eq!(found.len(), 1, "{method} {template}");
        let event = &found[0];
        assert!(envelope_errors(event).is_empty(), "{event}");
        assert_eq!(event["payload"]["endpoint"], template);
        assert_eq!(event["rsp_code"], response["rsp_code"]);
        if fallback {
            assert_eq!(event["payload"]["method"], method);
            assert_eq!(event["payload"]["http_status"], 200);
            assert!(event["payload"]["duration_ms"].as_u64().is_some());
            assert_eq!(
                event["channel"],
                if inbound { "EXTERNAL" } else { "MOBILE" }
            );
            assert_eq!(
                event["listener"],
                if inbound { "generator" } else { "public" }
            );
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
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-request-id", id)
        .header("content-type", content_type);
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
    assert_eq!(
        coverage.covered,
        contract_operations(),
        "every OpenAPI operation has a successful witness"
    );

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
            Some("unknown-token"),
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
    for path in ["/healthz", "/internal/v1/clock", "/internal/unknown"] {
        let id = new_id();
        call(&api.generator, "GET", path, &id, None, None).await;
        assert!(events(&api, &id).await.is_empty());
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
    let blocked = call(&api.public, "GET", &balance, &refused, Some(&a.token), None).await;
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
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(blocked.1["rsp_code"], "A0013");
    assert!(blocked_events.is_empty());
    assert_eq!(other.0, StatusCode::OK);
    assert_eq!(other_events.len(), 1);

    // T6: inspect the complete OUTBOX, including handler-owned events.
    let rows: Vec<String> = api.state.db.run(|s| s.query_all(&ALL, &[])).await.unwrap();
    for text in rows {
        for secret in [
            password,
            "outcome-new-password",
            code,
            login_token,
            &a.token,
            &b.token,
            client_token,
            secret,
        ] {
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
