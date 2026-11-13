//! Recipient resolution, registration lifecycle and transactional events.
mod common;
use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{
    api, assert_contract, bank_tran_id, call, customer, envelope_errors, new_id, seed_counterparty,
    unique_digits,
};
use serde_json::{Value, json};
const META: SqlStmt = sql_stmt!("test.payee_metadata", "sql/payee_metadata.sql");
const EVENTS: SqlStmt = sql_stmt!("test.payee_events", "sql/payee_events.sql");
const ACCOUNT_ID: SqlStmt = sql_stmt!("test.payee_account_id", "sql/payee_account_id.sql");
const REQUESTS: SqlStmt = sql_stmt!("test.request_count", "sql/request_count.sql");
#[tokio::test]
async fn recipients_and_payee_lifecycle() {
    let api = api().await;
    let a = customer(&api, 1000).await;
    let b = customer(&api, 0).await;
    let external = format!("95{}", unique_digits(10));
    seed_counterparty(&api, &external).await;
    for (bank, name) in [("901", "가".repeat(51)), ("000", "unknown".to_owned())] {
        let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/counterparties", &new_id(), None,
            Some(json!({"counterparties": [{"counterparty_type": "INDIVIDUAL", "institution_code": bank,
                "account_no": unique_digits(12), "name": name}]}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_contract("Error", &body);
    }
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None,
        Some(json!({"customers": [{"customer_no": format!("C{}", unique_digits(10)), "phone": format!("010{}", unique_digits(8)),
            "name": "가".repeat(51), "password": "pw-payees", "customer_type": "INDIVIDUAL"}]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_contract("Error", &body);
    let now = api.state.clock.state().await.unwrap().business_time;
    for (bank, account, holder) in [
        ("998", b.account.as_str(), Some("김수취")),
        ("901", external.as_str(), Some("외부수취인")),
        ("998", "1", None),
        ("901", "1", None),
        ("000", "1", None),
    ] {
        let rid = new_id();
        let btid = bank_tran_id();
        let (status, body) = call(
            &api.public,
            "POST",
            "/v2.0/inquiry/receive",
            &rid,
            Some(&a.token),
            Some(
                json!({"bank_tran_id": btid, "bank_code_std": bank, "account_num": account,
                "tran_amt": "123", "tran_dtime": now.format("%Y%m%d%H%M%S").to_string()}),
            ),
        )
        .await;
        assert_eq!(body["bank_tran_id"], btid);
        assert_eq!(body["bank_tran_date"], now.format("%Y%m%d").to_string());
        if let Some(holder) = holder {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_contract("InquiryReceiveResponse", &body);
            assert_eq!(body["account_holder_name"], holder);
            assert_eq!(body["bank_code_std"], bank);
            assert_eq!(body["tran_amt"], "123");
            assert_eq!(
                body["account_num_masked"],
                bank::shared::api::fields::mask_account(account)
            );
        } else {
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_contract("BankRejected", &body);
            assert_eq!(body["bank_rsp_code"], "404");
        }
        let count: i64 = api
            .state
            .db
            .run(move |s| s.query_row(&REQUESTS, &[&rid]))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let first = common::register_payee(&api, &a.token, "998", &b.account).await;
    let second = common::register_payee(&api, &a.token, "901", &external).await;
    assert_eq!(first["alias"], "수취인 별칭");
    let id = first["payee_id"].as_str().unwrap().to_owned();
    let (status, body) = call(
        &api.public,
        "GET",
        "/v2.0/ext/payees",
        &new_id(),
        Some(&a.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_contract("PayeeListResponse", &body);
    assert_eq!(body["res_cnt"], "2");
    assert_eq!(body["res_list"][0]["payee_id"], second["payee_id"]);
    assert_eq!(body["res_list"][1]["payee_id"], first["payee_id"]);
    let (status, duplicate) = call(
        &api.public,
        "POST",
        "/v2.0/ext/payees",
        &new_id(),
        Some(&a.token),
        Some(json!({"bank_code_std": "998", "account_num": b.account})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_contract("Error", &duplicate);
    assert_eq!(duplicate["rsp_code"], "A0007");
    let path = format!("/v2.0/ext/payees?payee_id={id}");
    let (status, body) = call(
        &api.public,
        "DELETE",
        &path,
        &new_id(),
        Some(&b.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_contract("Error", &body);
    let key = id.clone();
    let hash = bank::shared::auth::token_hash(&a.token);
    let (channel, session, expected): (String, String, String) = api
        .state
        .db
        .run(move |s| s.query_row(&META, &[&key, &hash]))
        .await
        .unwrap();
    assert_eq!(channel, "MOBILE");
    assert_eq!(session, expected);
    for expected in [StatusCode::OK, StatusCode::NOT_FOUND] {
        let (status, body) = call(
            &api.public,
            "DELETE",
            &path,
            &new_id(),
            Some(&a.token),
            None,
        )
        .await;
        assert_eq!(status, expected);
        assert_contract(
            if expected == StatusCode::OK {
                "Envelope"
            } else {
                "Error"
            },
            &body,
        );
    }
    let (status, body) = call(
        &api.public,
        "GET",
        "/v2.0/ext/payees",
        &new_id(),
        Some(&a.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_contract("PayeeListResponse", &body);
    assert_eq!(body["res_cnt"], "1");
    assert_eq!(body["res_list"][0]["payee_id"], second["payee_id"]);
    common::register_payee(&api, &a.token, "998", &b.account).await;
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENTS, &[&id]))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    for (raw, expected) in events.iter().zip(["payee.registered", "payee.deleted"]) {
        let event: Value = serde_json::from_str(raw).unwrap();
        assert!(envelope_errors(&event).is_empty(), "{event}");
        assert_eq!(event["event_type"], expected);
        if expected == "payee.registered" {
            let occurred =
                chrono::DateTime::parse_from_rfc3339(event["occurred_at"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                first["registered_dtime"],
                occurred.format("%Y%m%d%H%M%S").to_string()
            );
        }
        assert_eq!(event["payload"]["bank_code_std"], "998");
        assert_eq!(event["payload"]["account_num"], b.account);
    }
    // Deleted and nonexistent IDs are both hidden from the customer.
    let (status, body) = call(
        &api.public,
        "DELETE",
        "/v2.0/ext/payees?payee_id=0",
        &new_id(),
        Some(&a.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_contract("Error", &body);
    let account = b.account.clone();
    api.state
        .db
        .run(move |s| {
            let id: i64 = s.query_row(&ACCOUNT_ID, &[&account])?;
            let mut locks = bank::shared::ledger::lock_accounts(s, &[id])?;
            bank::shared::ledger::set_status(
                s,
                &mut locks,
                id,
                "CLOSED",
                "payee_test",
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
    for (bank, account, code) in [
        ("998", b.account.as_str(), "403"),
        ("998", "1", "404"),
        ("901", "1", "404"),
        ("000", "1", "404"),
    ] {
        let (status, body) = call(
            &api.public,
            "POST",
            "/v2.0/ext/payees",
            &new_id(),
            Some(&a.token),
            Some(json!({"bank_code_std": bank, "account_num": account, "channel": "SYSTEM"})),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_contract("PayeeRejected", &body);
        assert_eq!(body["bank_rsp_code"], code);
    }
    let (status, body) = call(
        &api.public,
        "POST",
        "/v2.0/inquiry/receive",
        &new_id(),
        Some(&a.token),
        Some(
            json!({"bank_tran_id": bank_tran_id(), "bank_code_std": "998", "account_num": b.account,
            "tran_amt": "1", "tran_dtime": now.format("%Y%m%d%H%M%S").to_string()}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_contract("BankRejected", &body);
    assert_eq!(body["bank_rsp_code"], "403");
}
