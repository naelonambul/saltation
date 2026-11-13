//! Step 6 proof: user info, account list, balance and transaction history
//! follow the KFTC shapes in contracts/openapi/public.yaml, paginate 25 rows
//! per page with a trace cursor, filter by direction, and never expose another
//! customer's account.

mod common;

use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use chrono::{DateTime, Duration, FixedOffset};
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
async fn insert_rows(
    api: &common::Api,
    account_no: &str,
    count: i64,
    now: DateTime<FixedOffset>,
) -> i64 {
    let account_no = account_no.to_owned();
    api.state
        .db
        .run(move |s| {
            let account_id: i64 = s.query_row(&ACCOUNT_ID, &[&account_no])?;
            let request_id = new_id();
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

fn history_path(
    now: DateTime<FixedOffset>,
    fin: &str,
    inquiry_type: &str,
    sort: &str,
    trace: Option<&str>,
) -> String {
    let mut p = format!(
        "/v2.0/account/transaction_list/fin_num?bank_tran_id={}&fintech_use_num={fin}&inquiry_type={inquiry_type}\
         &inquiry_base=D&from_date={}&to_date={}&sort_order={sort}&tran_dtime={}",
        bank_tran_id(),
        (now - Duration::days(1)).format("%Y%m%d"),
        now.format("%Y%m%d"),
        now.format("%Y%m%d%H%M%S")
    );
    if let Some(t) = trace {
        p.push_str(&format!("&befor_inquiry_trace_info={t}"));
    }
    p
}

#[tokio::test]
async fn inquiries_follow_the_contract() {
    let api = api().await;
    let now = api.state.clock.state().await.expect("clock").business_time;
    let tran_dtime = now.format("%Y%m%d%H%M%S").to_string();
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
            "/v2.0/account/balance/fin_num?bank_tran_id={}&fintech_use_num={fin}&tran_dtime={tran_dtime}",
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
        "/v2.0/account/balance/fin_num?bank_tran_id=BAD&fintech_use_num={fin}&tran_dtime={tran_dtime}"
    );
    let (status, body) = call(&api.generator, "GET", &bad, &new_id(), Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["rsp_code"], "A0002");

    // 30 rows: page 1 has 25 newest first, page 2 the remaining 5.
    let expected_balance = insert_rows(&api, &me.accounts[0], 30, now).await;
    let (status, page1) = call(
        &api.generator,
        "GET",
        &history_path(now, &fin, "A", "D", None),
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
        &history_path(now, &fin, "A", "D", Some(&trace)),
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
        &history_path(now, &fin, "A", "A", None),
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
        &history_path(now, &fin, "I", "D", None),
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

#[path = "common/loans.rs"]
#[allow(dead_code, unused_imports)]
mod loans;

async fn product_history(
    api: &common::Api,
    token: &str,
    fin: &str,
    now: DateTime<FixedOffset>,
    expected: &[(i64, i64)],
) {
    let body = common::deposits::request(
        api,
        token,
        "GET",
        &history_path(now, fin, "A", "A", None),
        None,
        StatusCode::OK,
        "TransactionListResponse",
    )
    .await;
    let fin = fin.to_owned();
    let stored: Vec<(i64, i64, String)> = api
        .state
        .db
        .run(move |s| {
            s.query_all(
                &sql_stmt!(
                    "test.accounts_product_history",
                    "sql/accounts_product_history.sql"
                ),
                &[&fin],
            )
        })
        .await
        .unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|(amount, after, _)| (*amount, *after))
            .collect::<Vec<_>>(),
        expected
    );
    let rows = body["res_list"].as_array().unwrap();
    assert_eq!(rows.len(), expected.len());
    assert_eq!(body["balance_amt"], expected.last().unwrap().1.to_string());
    for (row, (amount, after, direction)) in rows.iter().zip(stored) {
        assert_eq!(row["tran_amt"], amount.abs().to_string());
        assert_eq!(row["after_balance_amt"], after.to_string());
        assert_eq!(row["inout_type"], direction);
    }
}

#[tokio::test]
async fn product_accounts_support_inquiries() {
    use common::deposits::{close_payload, open_payload, request};
    let api = api().await;
    let me = common::customer(&api, 2_000_000).await;
    let other = common::customer(&api, 0).await;
    let now = api.state.clock.state().await.unwrap().business_time;
    let deposit = request(
        &api,
        &me.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(open_payload(&api, &me.fin, 100_000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let loan = request(
        &api,
        &me.token,
        "POST",
        "/v2.0/ext/loans",
        Some(loans::payload(&api, Some(&me.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    let deposit_fin = deposit["fintech_use_num"].as_str().unwrap();
    let loan_fin = loan["fintech_use_num"].as_str().unwrap();
    let balance_path = |fin: &str| {
        format!(
            "/v2.0/account/balance/fin_num?bank_tran_id={}&fintech_use_num={fin}&tran_dtime={}",
            bank_tran_id(),
            now.format("%Y%m%d%H%M%S")
        )
    };
    let list = request(
        &api,
        &me.token,
        "GET",
        "/v2.0/account/list",
        None,
        StatusCode::OK,
        "AccountListResponse",
    )
    .await;
    assert_eq!(list["res_cnt"], "3");
    let accounts = list["res_list"].as_array().unwrap();
    assert_eq!(accounts.len(), 3);
    for (fin, kind, id, agreement) in [
        (me.fin.as_str(), "1", None, "Y"),
        (deposit_fin, "2", Some(&deposit["deposit_id"]), "N"),
        (loan_fin, "4", Some(&loan["loan_id"]), "N"),
    ] {
        let account = accounts
            .iter()
            .find(|a| a["fintech_use_num"] == fin)
            .unwrap();
        assert_eq!(account["account_type"], kind);
        assert_eq!(account["account_state"], "01");
        assert_eq!(account["transfer_agree_yn"], agreement);
        assert_eq!(account.get("ext_account_id"), id);
    }
    for (fin, amount, maturity, kind, available) in [
        (me.fin.as_str(), "2900000", json!(""), "1", "2900000"),
        (
            deposit_fin,
            "100000",
            deposit["maturity_date"].clone(),
            "2",
            "0",
        ),
        (loan_fin, "1000000", loan["maturity_date"].clone(), "4", "0"),
    ] {
        let body = request(
            &api,
            &me.token,
            "GET",
            &balance_path(fin),
            None,
            StatusCode::OK,
            "BalanceResponse",
        )
        .await;
        assert_eq!(body["balance_amt"], amount);
        assert_eq!(body["available_amt"], available);
        assert_eq!(body["maturity_date"], maturity);
        assert_eq!(body["account_type"], kind);
    }
    product_history(&api, &me.token, deposit_fin, now, &[(100_000, 100_000)]).await;
    product_history(&api, &me.token, loan_fin, now, &[(1_000_000, 1_000_000)]).await;
    request(
        &api,
        &me.token,
        "POST",
        &format!(
            "/v2.0/ext/deposits/{}/close",
            deposit["deposit_id"].as_str().unwrap()
        ),
        Some(close_payload(&api).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    request(
        &api,
        &me.token,
        "POST",
        &format!(
            "/v2.0/ext/loans/{}/repay",
            loan["loan_id"].as_str().unwrap()
        ),
        Some(loans::payload(&api, None, 200_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    product_history(
        &api,
        &me.token,
        deposit_fin,
        now,
        &[(100_000, 100_000), (-100_000, 0)],
    )
    .await;
    product_history(
        &api,
        &me.token,
        loan_fin,
        now,
        &[(1_000_000, 1_000_000), (-200_000, 800_000)],
    )
    .await;
    for (fin, amount, maturity) in [
        (deposit_fin, "0", &deposit["maturity_date"]),
        (loan_fin, "800000", &loan["maturity_date"]),
    ] {
        let body = request(
            &api,
            &me.token,
            "GET",
            &balance_path(fin),
            None,
            StatusCode::OK,
            "BalanceResponse",
        )
        .await;
        assert_eq!(body["balance_amt"], amount);
        assert_eq!(body["available_amt"], "0");
        assert_eq!(&body["maturity_date"], maturity);
        for path in [balance_path(fin), history_path(now, fin, "A", "A", None)] {
            request(
                &api,
                &other.token,
                "GET",
                &path,
                None,
                StatusCode::NOT_FOUND,
                "Error",
            )
            .await;
        }
    }
    let list = request(
        &api,
        &me.token,
        "GET",
        "/v2.0/account/list",
        None,
        StatusCode::OK,
        "AccountListResponse",
    )
    .await;
    assert_eq!(list["res_cnt"], "3");
    let closed = list["res_list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["fintech_use_num"] == deposit_fin)
        .unwrap();
    assert_eq!(closed["account_state"], "09");
    assert_eq!(closed["ext_account_id"], deposit["deposit_id"]);
    assert_eq!(closed["transfer_agree_yn"], "N");
}
