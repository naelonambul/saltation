//! Generator inbound credential, isolation, idempotency and ledger proofs.
mod common;
use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::*;
use serde_json::{Value, json};
const ENDPOINT: &str = "/v2.0/transfer/deposit/fin_num";
const ACCOUNT: SqlStmt = sql_stmt!("test.inbound_account", "sql/inbound_account.sql");
const ROWS: SqlStmt = sql_stmt!("test.inbound_rows", "sql/inbound_rows.sql");
const EVENT: SqlStmt = sql_stmt!("test.inbound_event", "sql/inbound_event.sql");
const TOKEN: SqlStmt = sql_stmt!("test.inbound_token", "sql/inbound_token.sql");
type Snapshot = (
    i64,
    i64,
    String,
    Option<chrono::DateTime<chrono::FixedOffset>>,
);
async fn snapshot(api: &Api, account: &str) -> Snapshot {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&ACCOUNT, &[&account]))
        .await
        .unwrap()
}
async fn status(api: &Api, account: &str, target: &'static str) {
    let id = snapshot(api, account).await.0;
    let now = api.state.clock.state().await.unwrap().business_time;
    api.state
        .db
        .run(move |s| {
            let mut locks = bank::shared::ledger::lock_accounts(s, &[id])?;
            bank::shared::ledger::set_status(
                s,
                &mut locks,
                id,
                target,
                "inbound_test",
                now,
                None,
                "SYSTEM",
            )
            .map_err(|e| match e {
                bank::shared::ledger::LedgerError::Db(e) => e,
                e => bank::shared::db::DbError::Config(e.to_string()),
            })?;
            s.commit()
        })
        .await
        .unwrap();
}
fn payload(fin: &str, sender: &str, time: &str) -> Value {
    json!({"cntr_account_type": "N", "cntr_account_num": sender, "wd_print_content": "송금", "name_check_option": "off",
        "tran_dtime": time, "req_cnt": "1", "req_list": [{"tran_no": "1", "bank_tran_id": bank_tran_id(),
        "fintech_use_num": fin, "print_content": "입금", "tran_amt": "123", "req_client_name": "외부수취인", "transfer_purpose": "TR"}]})
}
async fn deposit(api: &Api, token: Option<&str>, req: Value) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        ENDPOINT,
        &new_id(),
        token,
        Some(req),
    )
    .await
}
fn core(mut response: Value) -> Value {
    for field in ["api_tran_id", "api_tran_dtm", "request_id"] {
        response.as_object_mut().unwrap().remove(field);
    }
    response
}
#[tokio::test]
async fn inbound_credentials_and_deposits() {
    let api = api().await;
    let client = format!("inbound_{}", new_id());
    let secret = "inbound-secret-123456";
    let seed = json!({"counterparty_clients": [{"client_id": client, "client_secret": secret, "institution_code": "901", "scope": "oob"}]});
    for expected in [1, 0] {
        let (status, body) = call(
            &api.generator,
            "POST",
            "/internal/v1/seed/counterparty_clients",
            &new_id(),
            None,
            Some(seed.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_contract("CreatedResponse", &body);
        assert_eq!(body["created"], expected);
    }
    for (field, value) in [
        ("client_secret", "123456789012345"),
        ("client_id", "bad!"),
        ("institution_code", "000"),
        ("scope", "card"),
    ] {
        let mut bad = seed.clone();
        bad["counterparty_clients"][0][field] = json!(value);
        let (status, body) = call(
            &api.generator,
            "POST",
            "/internal/v1/seed/counterparty_clients",
            &new_id(),
            None,
            Some(bad),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_contract("Error", &body);
        assert_eq!(body["rsp_code"], "A0002");
    }
    for (id, password) in [(client.as_str(), "wrong"), ("unknown-client", secret)] {
        let (status, body) = form_call(
            &api.generator,
            "/oauth/2.0/token",
            format!(
                "client_id={id}&client_secret={password}&scope=oob&grant_type=client_credentials"
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["rsp_code"], "O0001");
        assert_inbound_contract("Error", &body);
    }
    for (scope, grant, expected, code) in [
        ("card", "client_credentials", StatusCode::FORBIDDEN, "O0003"),
        ("oob", "password", StatusCode::BAD_REQUEST, "A0002"),
    ] {
        let (status, body) = form_call(
            &api.generator,
            "/oauth/2.0/token",
            format!("client_id={client}&client_secret={secret}&scope={scope}&grant_type={grant}"),
        )
        .await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(body["rsp_code"], code);
        assert_inbound_contract("Error", &body);
    }
    let before_token = api.state.clock.state().await.unwrap().business_time;
    let (http, body) = form_call(
        &api.generator,
        "/oauth/2.0/token",
        format!(
            "client_id={client}&client_secret={secret}&scope=oob&grant_type=client_credentials"
        ),
    )
    .await;
    assert_eq!(http, StatusCode::OK, "{body}");
    assert_inbound_contract("TokenResponse", &body);
    assert_eq!(body["expires_in"], 7776000);
    assert_eq!(body["client_use_code"], client);
    let token = body["access_token"].as_str().unwrap();
    let hash = bank::shared::auth::token_hash(token);
    let (subject, id, scope, session, expires): (
        String,
        String,
        String,
        Option<String>,
        chrono::DateTime<chrono::FixedOffset>,
    ) = api
        .state
        .db
        .run(move |s| s.query_row(&TOKEN, &[&hash]))
        .await
        .unwrap();
    assert_eq!(subject, "CLIENT");
    assert_eq!(id, client);
    assert_eq!(scope, "oob");
    assert_eq!(session, None);
    let after_token = api.state.clock.state().await.unwrap().business_time;
    assert!(expires >= before_token + chrono::Duration::days(90));
    assert!(expires <= after_token + chrono::Duration::days(90));
    let receiver = customer(&api, 0).await;
    let sender = format!("94{}", unique_digits(10));
    seed_counterparty(&api, &sender).await;
    let time = after_token.format("%Y%m%d%H%M%S").to_string();
    let req = payload(&receiver.fin, &sender, &time);
    for bearer in [None, Some(receiver.token.as_str()), Some("unknown-token")] {
        let (status, body) = deposit(&api, bearer, req.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_inbound_contract("Error", &body);
    }
    let (http, body) = call(
        &api.public,
        "GET",
        "/v2.0/ext/payees",
        &new_id(),
        Some(token),
        None,
    )
    .await;
    assert_eq!(http, StatusCode::UNAUTHORIZED);
    assert_contract("Error", &body);
    let before = snapshot(&api, &receiver.account).await;
    let rid = new_id();
    let (http, body) = call(
        &api.generator,
        "POST",
        ENDPOINT,
        &rid,
        Some(token),
        Some(req.clone()),
    )
    .await;
    assert_eq!(http, StatusCode::OK, "{body}");
    assert_inbound_contract("DepositResponse", &body);
    assert_eq!(body["res_list"][0]["bank_rsp_code"], "000");
    assert_eq!(body["wd_bank_code_std"], "901");
    assert_eq!(body["wd_bank_name"], "가람은행");
    assert_eq!(body["wd_account_holder_name"], "외부수취인");
    assert_eq!(
        body["wd_account_num_masked"],
        bank::shared::api::fields::mask_account(&sender)
    );
    assert_eq!(body["res_list"][0]["account_holder_name"], "김수취");
    assert_eq!(body["wd_print_content"], "송금");
    assert_eq!(body["res_list"][0]["print_content"], "입금");
    let after = snapshot(&api, &receiver.account).await;
    assert_eq!(after.1, before.1 + 123);
    assert_eq!(after.3, before.3);
    let event_id = rid.clone();
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENT, &[&event_id]))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    let event: Value = serde_json::from_str(&events[0]).unwrap();
    assert!(envelope_errors(&event).is_empty(), "{event}");
    assert_eq!(event["channel"], "EXTERNAL");
    assert_eq!(event["event_type"], "request.completed");
    let r = rid.clone();
    let rows: Vec<(i64, String, String, String, String)> = api
        .state
        .db
        .run(move |s| s.query_all(&ROWS, &[&r]))
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![(
            123,
            "901".into(),
            sender.clone(),
            "외부수취인".into(),
            "inbound".into()
        )]
    );
    let (http, replay) = deposit(&api, Some(token), req.clone()).await;
    assert_eq!(http, StatusCode::OK);
    assert_eq!(core(replay), core(body));
    assert_eq!(snapshot(&api, &receiver.account).await, after);
    let rows: Vec<(i64, String, String, String, String)> = api
        .state
        .db
        .run(move |s| s.query_all(&ROWS, &[&rid]))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let mut conflict = req.clone();
    conflict["req_list"][0]["tran_amt"] = json!("124");
    let (http, body) = deposit(&api, Some(token), conflict).await;
    assert_eq!(http, StatusCode::CONFLICT);
    assert_inbound_contract("Error", &body);
    assert_eq!(body["rsp_code"], "A0007");
    assert_eq!(snapshot(&api, &receiver.account).await, after);
    let mut wrong_sender = payload(&receiver.fin, &sender, &time);
    wrong_sender["cntr_account_num"] = json!(receiver.account);
    let (http, body) = deposit(&api, Some(token), wrong_sender).await;
    assert_eq!(http, StatusCode::BAD_REQUEST);
    assert_eq!(body["rsp_code"], "A0002");
    assert_inbound_contract("Error", &body);
    for (field, value) in [
        ("cntr_account_type", json!("X")),
        ("req_cnt", json!("2")),
        ("wd_print_content", json!("가".repeat(21))),
    ] {
        let mut bad = payload(&receiver.fin, &sender, &time);
        bad[field] = value;
        let (http, body) = deposit(&api, Some(token), bad).await;
        assert_eq!(http, StatusCode::BAD_REQUEST);
        assert_inbound_contract("Error", &body);
    }
    let unknown = payload(
        &format!("{}{}", unique_digits(12), unique_digits(12)),
        &sender,
        &time,
    );
    {
        let req = unknown;
        let (http, body) = deposit(&api, Some(token), req.clone()).await;
        assert_eq!(http, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_inbound_contract("DepositResponse", &body);
        assert_eq!(body["res_list"][0]["bank_rsp_code"], "404");
        assert!(body["res_list"][0].get("account_num_masked").is_none());
        assert!(body["res_list"][0].get("account_holder_name").is_none());
        let (http, replay) = deposit(&api, Some(token), req).await;
        assert_eq!(http, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(core(body), core(replay));
    }
    let dormant = customer(&api, 0).await;
    status(&api, &dormant.account, "DORMANT").await;
    let before = snapshot(&api, &dormant.account).await;
    let (http, body) = deposit(&api, Some(token), payload(&dormant.fin, &sender, &time)).await;
    assert_eq!(http, StatusCode::OK, "{body}");
    assert_inbound_contract("DepositResponse", &body);
    let after = snapshot(&api, &dormant.account).await;
    assert_eq!(after.1, before.1 + 123);
    assert_eq!(after.2, "DORMANT");
    assert_eq!(after.3, before.3);
    let closed = customer(&api, 0).await;
    status(&api, &closed.account, "CLOSED").await;
    let before = snapshot(&api, &closed.account).await;
    let req = payload(&closed.fin, &sender, &time);
    let (http, body) = deposit(&api, Some(token), req.clone()).await;
    assert_eq!(http, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_inbound_contract("DepositResponse", &body);
    assert_eq!(body["res_list"][0]["bank_rsp_code"], "403");
    let (http, replay) = deposit(&api, Some(token), req).await;
    assert_eq!(http, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(core(body), core(replay.clone()));
    assert_eq!(snapshot(&api, &closed.account).await, before);
    let request_id = replay["request_id"].as_str().unwrap().to_owned();
    // The replay request receives its own fallback completion event.
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENT, &[&request_id]))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    let event: Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(event["outcome"], "rejected");
    assert_eq!(event["rsp_code"], replay["rsp_code"]);
    assert_eq!(event["bank_rsp_code"], "403");
    assert_eq!(event["channel"], "EXTERNAL");
    assert_eq!(event["listener"], "generator");
    assert_eq!(event["payload"]["endpoint"], ENDPOINT);
    assert_eq!(event["payload"]["method"], "POST");
    assert_eq!(event["payload"]["http_status"], 422);
    assert!(event["payload"]["duration_ms"].as_u64().is_some());
}
