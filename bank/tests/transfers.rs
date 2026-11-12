//! Step 7 proof: in-bank transfers, rejections, the daily limit, idempotency
//! (replay, conflicting payload, 200 concurrent duplicates) and concurrency
//! (1,000 random two-way transfers on 32 tasks: no deadlock errors, total
//! balance unchanged, every transfer offsets, every balance equals its rows).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{Api, api, assert_contract, bank_tran_id, call, new_id, unique_digits};
use serde_json::{Value, json};

const SUM_BALANCES: SqlStmt = sql_stmt!("test.sum_balances", "sql/sum_balances.sql");
const BAD_TRANSFERS: SqlStmt = sql_stmt!("test.bad_transfers", "sql/bad_transfers.sql");
const BAD_BALANCES: SqlStmt = sql_stmt!("test.bad_balances", "sql/bad_balances.sql");
const BAD_RUNNING: SqlStmt = sql_stmt!("test.bad_running_balances", "sql/bad_running_balances.sql");
const COUNT_BTID: SqlStmt = sql_stmt!("test.count_transfers_btid", "sql/count_transfers_btid.sql");
const REQUEST_STATUS: SqlStmt = sql_stmt!("test.request_status", "sql/request_status.sql");

struct Customer {
    token: String,
    accounts: Vec<(String, String)>, // (account_no, fintech_use_num)
    prefix: String,
}

async fn customer(api: &Api, accounts: usize, opening: i64) -> Customer {
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None, Some(json!({
        "customers": [{ "customer_no": customer_no, "name": "박송금", "phone": phone, "address": "대구",
                        "customer_type": "INDIVIDUAL", "password": "pw-transfer" }]
    })))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let prefix = format!("97{n}");
    let list: Vec<Value> = (0..accounts)
        .map(|i| json!({ "customer_no": customer_no, "account_no": format!("{prefix}{i:02}"), "opening_balance": opening }))
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
    let accounts = body["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .map(|a| {
            (
                a["account_no"].as_str().expect("no").to_owned(),
                a["fintech_use_num"].as_str().expect("fin").to_owned(),
            )
        })
        .collect();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({ "phone": phone, "password": "pw-transfer", "device_id": "dev-t" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Customer {
        token: body["access_token"].as_str().expect("token").to_owned(),
        accounts,
        prefix,
    }
}

fn transfer_body(btid: &str, from_fin: &str, to_account: &str, amount: i64) -> Value {
    json!({
        "bank_tran_id": btid, "fintech_use_num": from_fin, "tran_amt": amount.to_string(),
        "tran_dtime": "20260101000000", "transfer_purpose": "TR",
        "recv_client_name": "수취인", "recv_client_bank_code": "998", "recv_client_account_num": to_account,
    })
}

async fn transfer(api: &Api, token: &str, body: Value) -> (StatusCode, Value, String) {
    let r = new_id();
    let (status, json) = call(
        &api.generator,
        "POST",
        "/v2.0/transfer/withdraw/fin_num",
        &r,
        Some(token),
        Some(body),
    )
    .await;
    (status, json, r)
}

async fn scalar(api: &Api, stmt: &'static SqlStmt, param: Option<String>) -> i64 {
    api.state
        .db
        .run(move |s| match &param {
            Some(p) => s.query_row(stmt, &[p]),
            None => s.query_row(stmt, &[]),
        })
        .await
        .expect("scalar")
}

async fn text(api: &Api, stmt: &'static SqlStmt, param: String) -> String {
    api.state
        .db
        .run(move |s| s.query_row(stmt, &[&param]))
        .await
        .expect("text")
}

async fn assert_consistent(api: &Api) {
    assert_eq!(
        scalar(api, &BAD_TRANSFERS, None).await,
        0,
        "every transfer's two rows offset"
    );
    assert_eq!(
        scalar(api, &BAD_BALANCES, None).await,
        0,
        "every balance equals the sum of its rows"
    );
    assert_eq!(
        scalar(api, &BAD_RUNNING, None).await,
        0,
        "every balance_after is the running sum"
    );
}

#[tokio::test]
async fn transfers_rejections_limit_and_idempotency() {
    let api = api().await;
    let a = customer(&api, 2, 60_000_000).await;
    let b = customer(&api, 1, 0).await;
    let (a1, a1_fin) = a.accounts[0].clone();
    let (b1, b1_fin) = b.accounts[0].clone();

    // Success, with the contract shape and both rows.
    let btid = bank_tran_id();
    let (status, body, _) =
        transfer(&api, &a.token, transfer_body(&btid, &a1_fin, &b1, 10_000)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("TransferResponse", &body);
    assert_eq!(
        scalar(&api, &SUM_BALANCES, Some(format!("{}%", b.prefix))).await,
        10_000
    );

    // Replay: same key and payload returns the stored result; no second movement.
    let (status, replay, _) =
        transfer(&api, &a.token, transfer_body(&btid, &a1_fin, &b1, 10_000)).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["bank_tran_id"], body["bank_tran_id"]);
    assert_eq!(replay["wd_limit_remain_amt"], body["wd_limit_remain_amt"]);
    // Same key, different payload: conflict.
    let (status, conflict, _) =
        transfer(&api, &a.token, transfer_body(&btid, &a1_fin, &b1, 20_000)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["rsp_code"], "A0007");
    assert_eq!(scalar(&api, &COUNT_BTID, Some(btid.clone())).await, 1);

    // Insufficient balance (B has 10,000): rejected and recorded, no movement.
    let (status, body, r) = transfer(
        &api,
        &b.token,
        transfer_body(&bank_tran_id(), &b1_fin, &a1, 1_000_000),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_contract("TransferRejected", &body);
    assert_eq!(body["bank_rsp_code"], "405");
    assert_eq!(text(&api, &REQUEST_STATUS, r).await, "REJECTED/405");

    // Same account.
    let (status, body, _) = transfer(
        &api,
        &a.token,
        transfer_body(&bank_tran_id(), &a1_fin, &a1, 1),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["bank_rsp_code"], "403");

    // Unknown receiver.
    let (status, body, _) = transfer(
        &api,
        &a.token,
        transfer_body(&bank_tran_id(), &a1_fin, "1", 1),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["bank_rsp_code"], "404");

    // Someone else's account as the source.
    let (status, _, _) = transfer(
        &api,
        &b.token,
        transfer_body(&bank_tran_id(), &a1_fin, &b1, 1),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Daily limit 50,000,000: 10,000 already sent; 49,990,000 passes, 1 more fails.
    let (status, body, _) = transfer(
        &api,
        &a.token,
        transfer_body(&bank_tran_id(), &a1_fin, &b1, 49_990_000),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["wd_limit_remain_amt"], "0");
    let (status, body, _) = transfer(
        &api,
        &a.token,
        transfer_body(&bank_tran_id(), &a1_fin, &b1, 1),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["bank_rsp_code"], "406");

    // Transfer result by bank_tran_id.
    let (status, body) = call(&api.generator, "POST", "/v2.0/transfer/result", &new_id(), Some(&a.token), Some(json!({
        "check_type": "1", "tran_dtime": "20260101000000", "req_cnt": "1",
        "req_list": [{ "tran_no": "1", "org_bank_tran_id": btid, "org_bank_tran_date": "20260101", "org_tran_amt": "10000" }]
    })))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("TransferResultResponse", &body);
    assert_eq!(body["res_list"][0]["bank_rsp_code"], "000");

    assert_consistent(&api).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_duplicates_move_money_once() {
    let api = Arc::new(api().await);
    let a = customer(&api, 1, 1_000_000).await;
    let b = customer(&api, 1, 0).await;
    let btid = bank_tran_id();
    let body = transfer_body(&btid, &a.accounts[0].1, &b.accounts[0].0, 1_234);
    let mut tasks = Vec::new();
    for _ in 0..200 {
        let api = api.clone();
        let token = a.token.clone();
        let body = body.clone();
        tasks.push(tokio::spawn(
            async move { transfer(&api, &token, body).await.0 },
        ));
    }
    let mut ok = 0;
    for t in tasks {
        let status = t.await.expect("task");
        assert!(status == StatusCode::OK, "duplicate returned {status}");
        ok += 1;
    }
    assert_eq!(ok, 200);
    assert_eq!(
        scalar(&api, &COUNT_BTID, Some(btid)).await,
        1,
        "200 duplicates move money once"
    );
    assert_eq!(
        scalar(&api, &SUM_BALANCES, Some(format!("{}%", b.prefix))).await,
        1_234
    );
    assert_consistent(&api).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn random_two_way_transfers_keep_the_books_consistent() {
    let api = Arc::new(api().await);
    let c = Arc::new(customer(&api, 20, 1_000_000).await);
    let before = scalar(&api, &SUM_BALANCES, Some(format!("{}%", c.prefix))).await;
    assert_eq!(before, 20_000_000);

    let mut tasks = Vec::new();
    for worker in 0..32u64 {
        let api = api.clone();
        let c = c.clone();
        tasks.push(tokio::spawn(async move {
            let mut seed = worker
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let mut next = move || {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (seed >> 33) as usize
            };
            let mut statuses = Vec::new();
            for _ in 0..(1000 / 32 + 1) {
                let from = next() % 20;
                let mut to = next() % 20;
                if to == from {
                    to = (to + 1) % 20;
                }
                let amount = (next() % 5_000 + 1) as i64;
                let body = transfer_body(
                    &bank_tran_id(),
                    &c.accounts[from].1,
                    &c.accounts[to].0,
                    amount,
                );
                statuses.push(transfer(&api, &c.token, body).await.0);
            }
            statuses
        }));
    }
    let mut ok = 0;
    for t in tasks {
        for status in t.await.expect("task") {
            assert!(
                status == StatusCode::OK || status == StatusCode::UNPROCESSABLE_ENTITY,
                "unexpected status {status} (deadlock, timeout or server error)"
            );
            if status == StatusCode::OK {
                ok += 1;
            }
        }
    }
    assert!(ok >= 1000, "at least 1,000 transfers succeeded, got {ok}");
    let after = scalar(&api, &SUM_BALANCES, Some(format!("{}%", c.prefix))).await;
    assert_eq!(after, before, "money is neither created nor destroyed");
    assert_consistent(&api).await;
}
