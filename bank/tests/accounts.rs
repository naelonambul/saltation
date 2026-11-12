//! Step 6 proof: user info, account list, balance and transaction history
//! follow the KFTC shapes in contracts/openapi/public.yaml, paginate 25 rows
//! per page with a trace cursor, filter by direction, and never expose another
//! customer's account.

mod common;

use axum::http::StatusCode;
use bank::shared::clock::kst;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use chrono::{Duration, Utc};
use common::{api, assert_contract, bank_tran_id, call, contract_errors, new_id, unique_digits};
use serde_json::{Value, json};

const INSERT_REQUEST: SqlStmt =
    sql_stmt!("test.insert_test_request", "sql/insert_test_request.sql");
const INSERT_ROW: SqlStmt = sql_stmt!("test.insert_test_row", "sql/insert_test_row.sql");
const SET_BALANCE: SqlStmt = sql_stmt!("test.set_balance", "sql/set_balance.sql");
const ACCOUNT_ID: SqlStmt = sql_stmt!(
    "test.select_account_id_by_no",
    "sql/select_account_id_by_no.sql"
);

struct Customer {
    phone: String,
    accounts: Vec<String>,
}

async fn seed(api: &common::Api, accounts: usize) -> Customer {
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None, Some(json!({
        "customers": [{ "customer_no": customer_no, "name": "이조회", "phone": phone, "address": "부산",
                        "customer_type": "INDIVIDUAL", "password": "pw-123456" }]
    })))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let numbers: Vec<String> = (0..accounts).map(|i| format!("998{n}{i}")).collect();
    let list: Vec<Value> = numbers
        .iter()
        .map(|a| json!({ "customer_no": customer_no, "account_no": a }))
        .collect();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/accounts",
        &new_id(),
        None,
        Some(json!({ "accounts": list })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Customer {
        phone,
        accounts: numbers,
    }
}

async fn login(api: &common::Api, c: &Customer) -> String {
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({ "phone": c.phone, "password": "pw-123456", "device_id": "dev-q" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("LoginResponse", &body);
    body["access_token"].as_str().expect("token").to_owned()
}

/// Inserts `count` rows: odd sequences are deposits of 10,000, even ones withdrawals of 3,000.
async fn insert_rows(api: &common::Api, account_no: &str, count: i64) -> i64 {
    let account_no = account_no.to_owned();
    api.state
        .db
        .run(move |s| {
            let account_id: i64 = s.query_row(&ACCOUNT_ID, &[&account_no])?;
            let request_id = new_id();
            let now = Utc::now().with_timezone(&kst());
            s.execute(&INSERT_REQUEST, &[&request_id, &request_id, &now])?;
            let mut balance = 0i64;
            for seq in 1..=count {
                let amount = if seq % 2 == 1 { 10_000 } else { -3_000 };
                balance += amount;
                let inout = if amount > 0 { "입금" } else { "출금" };
                let at = now - Duration::minutes(count - seq);
                s.execute(
                    &INSERT_ROW,
                    &[
                        &account_id,
                        &seq,
                        &amount,
                        &balance,
                        &inout,
                        &format!("테스트{seq}"),
                        &request_id,
                        &at,
                        &at,
                    ],
                )?;
            }
            s.execute(&SET_BALANCE, &[&balance, &(count + 1), &now, &account_no])?;
            s.commit()?;
            Ok(balance)
        })
        .await
        .expect("insert rows")
}

fn today() -> String {
    Utc::now()
        .with_timezone(&kst())
        .format("%Y%m%d")
        .to_string()
}

fn yesterday() -> String {
    (Utc::now().with_timezone(&kst()) - Duration::days(1))
        .format("%Y%m%d")
        .to_string()
}

fn history_path(fin: &str, inquiry_type: &str, sort: &str, trace: Option<&str>) -> String {
    let mut p = format!(
        "/v2.0/account/transaction_list/fin_num?bank_tran_id={}&fintech_use_num={fin}&inquiry_type={inquiry_type}\
         &inquiry_base=D&from_date={}&to_date={}&sort_order={sort}&tran_dtime=20260101000000",
        bank_tran_id(),
        yesterday(),
        today()
    );
    if let Some(t) = trace {
        p.push_str(&format!("&befor_inquiry_trace_info={t}"));
    }
    p
}

#[tokio::test]
async fn inquiries_follow_the_contract() {
    let api = api().await;
    let me = seed(&api, 2).await;
    let other = seed(&api, 1).await;
    let token = login(&api, &me).await;
    let other_token = login(&api, &other).await;

    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/user/me",
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("UserMeResponse", &body);
    assert_eq!(body["res_cnt"], "2");
    let fin = body["res_list"][0]["fintech_use_num"]
        .as_str()
        .expect("fin")
        .to_owned();

    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("AccountListResponse", &body);

    let balance_path = |fin: &str| {
        format!(
            "/v2.0/account/balance/fin_num?bank_tran_id={}&fintech_use_num={fin}&tran_dtime=20260101000000",
            bank_tran_id()
        )
    };
    let (status, body) = call(
        &api.generator,
        "GET",
        &balance_path(&fin),
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("BalanceResponse", &body);
    assert_eq!(body["balance_amt"], "0");

    // Another customer cannot read this account.
    let (status, body) = call(
        &api.generator,
        "GET",
        &balance_path(&fin),
        &new_id(),
        Some(&other_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_contract("Error", &body);

    // Malformed bank_tran_id is rejected.
    let bad = format!(
        "/v2.0/account/balance/fin_num?bank_tran_id=BAD&fintech_use_num={fin}&tran_dtime=20260101000000"
    );
    let (status, body) = call(&api.generator, "GET", &bad, &new_id(), Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["rsp_code"], "A0002");

    // 30 rows: page 1 has 25 newest first, page 2 the remaining 5.
    let expected_balance = insert_rows(&api, &me.accounts[0], 30).await;
    let (status, page1) = call(
        &api.generator,
        "GET",
        &history_path(&fin, "A", "D", None),
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_contract("TransactionListResponse", &page1);
    assert_eq!(page1["page_record_cnt"], "25");
    assert_eq!(page1["next_page_yn"], "Y");
    assert_eq!(page1["balance_amt"], expected_balance.to_string());
    assert_eq!(
        page1["res_list"][0]["print_content"], "테스트30",
        "newest first"
    );
    let trace = page1["befor_inquiry_trace_info"]
        .as_str()
        .expect("trace")
        .to_owned();
    let (status, page2) = call(
        &api.generator,
        "GET",
        &history_path(&fin, "A", "D", Some(&trace)),
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_contract("TransactionListResponse", &page2);
    assert_eq!(page2["page_record_cnt"], "5");
    assert_eq!(page2["next_page_yn"], "N");
    assert_eq!(page2["res_list"][4]["print_content"], "테스트1");

    // Ascending order starts at the oldest row.
    let (_, asc) = call(
        &api.generator,
        "GET",
        &history_path(&fin, "A", "A", None),
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(asc["res_list"][0]["print_content"], "테스트1");

    // Deposits only.
    let (_, deposits) = call(
        &api.generator,
        "GET",
        &history_path(&fin, "I", "D", None),
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_contract("TransactionListResponse", &deposits);
    assert_eq!(deposits["page_record_cnt"], "15");
    assert!(
        deposits["res_list"]
            .as_array()
            .expect("list")
            .iter()
            .all(|r| r["inout_type"] == "입금")
    );

    // The contract check itself rejects malformed responses.
    let mut broken = page2.clone();
    broken["res_list"][0]["tran_amt"] = json!("-5");
    assert!(
        !contract_errors("TransactionListResponse", &broken).is_empty(),
        "a negative tran_amt must fail"
    );
    assert!(!contract_errors("BalanceResponse", &json!({ "rsp_code": "A0000" })).is_empty());

    // Without a token nothing is readable.
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/user/me",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_contract("Error", &body);
}
