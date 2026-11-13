//! Loan contract and ledger proofs using unique customers without clock changes.
mod common;
#[path = "common/loans.rs"]
mod loans;
use axum::http::StatusCode;
use bank::shared::clock::business_date;
use common::*;
use loans::*;
use serde_json::json;
const ROOT: &str = "/v2.0/ext/loans";

#[tokio::test]
async fn loans() {
    let api = api().await;
    let a = customer(&api, 500_000).await;
    let b = customer(&api, 0).await;
    transfer(&api, &a, &b, 123).await;
    let before = snapshot(&api, &a.fin).await;
    assert_eq!(before.1, 123);
    let take = payload(&api, Some(&a.fin), 1_000_000).await;
    let rid = new_id();
    let (status, opened) = call(
        &api.public,
        "POST",
        ROOT,
        &rid,
        Some(&a.token),
        Some(take.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_contract("LoanTranResponse", &opened);
    let take_rid = rid.clone();
    completion(&api, rid, "LOAN_TAKE").await;
    let id = opened["loan_id"].as_str().unwrap().parse::<i64>().unwrap();
    let fin = opened["fintech_use_num"].as_str().unwrap();
    let path = format!("{ROOT}/{id}");
    let repay_path = format!("{path}/repay");
    assert_eq!(
        snapshot(&api, &a.fin).await,
        (before.0 + 1_000_000, before.1)
    );
    assert_eq!(snapshot(&api, fin).await, (1_000_000, 0));
    assert_eq!(opened["principal_amt"], "1000000");
    assert_eq!(opened["balance_amt"], "1000000");
    assert_eq!(opened["account_state"], "01");
    assert_eq!(opened["linked_fintech_use_num"], a.fin);
    let now = api.state.clock.state().await.unwrap().business_time;
    assert_eq!(
        opened["account_issue_date"],
        business_date(now).format("%Y%m%d").to_string()
    );
    assert_eq!(
        opened["maturity_date"],
        business_date(now)
            .checked_add_months(chrono::Months::new(12))
            .unwrap()
            .format("%Y%m%d")
            .to_string()
    );
    let read = request(
        &api,
        &a.token,
        "GET",
        &path,
        None,
        StatusCode::OK,
        "LoanResponse",
    )
    .await;
    for key in [
        "fintech_use_num",
        "principal_amt",
        "balance_amt",
        "maturity_date",
        "account_state",
    ] {
        assert_eq!(read[key], opened[key]);
    }
    provenance(&api, id, &take_rid, "loan_disburse", 1_000_000).await;
    common::deposits::counterparties(&api, id, 2).await;
    let rows = movements(&api, id).await;
    assert_eq!(
        rows,
        vec![("loan_disburse".into(), 2, 1_000_000, 1_000_000, 4)]
    );
    let replay = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(take.clone()),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    assert_eq!(core(replay), core(opened.clone()));
    assert_eq!(movements(&api, id).await, rows);
    assert_eq!(
        snapshot(&api, &a.fin).await,
        (before.0 + 1_000_000, before.1)
    );
    let mut conflict = take;
    conflict["tran_amt"] = json!("1000001");
    request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(conflict),
        StatusCode::CONFLICT,
        "Error",
    )
    .await;
    let rejected = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&a.fin), 999_999).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(rejected["bank_rsp_code"], "407");
    let mut term = payload(&api, Some(&a.fin), 1_000_000).await;
    term["terms_id"] = json!("2");
    for req in [term, payload(&api, Some(&b.fin), 1_000_000).await] {
        let missing = request(
            &api,
            &a.token,
            "POST",
            ROOT,
            Some(req.clone()),
            StatusCode::NOT_FOUND,
            "Error",
        )
        .await;
        let replay = request(
            &api,
            &a.token,
            "POST",
            ROOT,
            Some(req),
            StatusCode::NOT_FOUND,
            "Error",
        )
        .await;
        assert_eq!(core(missing), core(replay));
    }
    request(
        &api,
        &b.token,
        "GET",
        &path,
        None,
        StatusCode::NOT_FOUND,
        "Error",
    )
    .await;
    request(
        &api,
        &b.token,
        "POST",
        &repay_path,
        Some(payload(&api, None, 1).await),
        StatusCode::NOT_FOUND,
        "Error",
    )
    .await;
    let partial = payload(&api, None, 200_000).await;
    let rid = new_id();
    let (status, repaid) = call(
        &api.public,
        "POST",
        &repay_path,
        &rid,
        Some(&a.token),
        Some(partial.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repaid}");
    assert_contract("LoanTranResponse", &repaid);
    provenance(&api, id, &rid, "loan_repay", -200_000).await;
    common::deposits::counterparties(&api, id, 4).await;
    completion(&api, rid, "LOAN_REPAY").await;
    assert_eq!(repaid["balance_amt"], "800000");
    assert_eq!(repaid["account_state"], "01");
    assert_eq!(snapshot(&api, &a.fin).await, (before.0 + 800_000, before.1));
    assert_eq!(snapshot(&api, fin).await.0, 800_000);
    let rows = movements(&api, id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1], ("loan_repay".into(), 2, -200_000, -200_000, 4));
    let replay = request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(partial.clone()),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    assert_eq!(core(replay), core(repaid));
    assert_eq!(movements(&api, id).await, rows);
    let mut conflict = partial;
    conflict["tran_amt"] = json!("200001");
    request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(conflict),
        StatusCode::CONFLICT,
        "Error",
    )
    .await;
    let over = payload(&api, None, 800_001).await;
    let rejected = request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(over.clone()),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(rejected["bank_rsp_code"], "408");
    let replay = request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(over),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(core(replay), core(rejected));
    assert_eq!(movements(&api, id).await, rows);
    assert_eq!(snapshot(&api, &a.fin).await, (before.0 + 800_000, before.1));
    assert_eq!(snapshot(&api, fin).await.0, 800_000);
    let rid = new_id();
    let full = payload(&api, None, 800_000).await;
    let (status, closed) = call(
        &api.public,
        "POST",
        &repay_path,
        &rid,
        Some(&a.token),
        Some(full.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_contract("LoanTranResponse", &closed);
    assert_eq!(closed["balance_amt"], "0");
    assert_eq!(closed["account_state"], "09");
    assert_eq!(snapshot(&api, fin).await.0, 0);
    assert_eq!(snapshot(&api, &a.fin).await, before);
    let closure: (String, i64, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&CLOSURE, &[&rid, &id]))
        .await
        .unwrap();
    assert_eq!(closure, ("CLOSED".into(), 1, 1));
    let rejected = request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(payload(&api, None, 1).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(rejected["bank_rsp_code"], "403");
    let replay = request(
        &api,
        &a.token,
        "POST",
        &repay_path,
        Some(full),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    assert_eq!(core(replay), core(closed));
    let read = request(
        &api,
        &a.token,
        "GET",
        &path,
        None,
        StatusCode::OK,
        "LoanResponse",
    )
    .await;
    assert_eq!(read["balance_amt"], "0");
    assert_eq!(read["account_state"], "09");

    // A second loan's receiver is drained by a term deposit, without changing transfer usage.
    let second = request(
        &api,
        &b.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&b.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    let second_fin = second["fintech_use_num"].as_str().unwrap();
    let second_path = format!("{ROOT}/{}/repay", second["loan_id"].as_str().unwrap());
    let mut drain = payload(&api, Some(&b.fin), 1_000_123).await;
    drain["terms_id"] = json!("2");
    request(
        &api,
        &b.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(drain),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let before_b = snapshot(&api, &b.fin).await;
    let before_loan = snapshot(&api, second_fin).await;
    assert_eq!(before_b.0, 0);
    let no_movement = movements(&api, second["loan_id"].as_str().unwrap().parse().unwrap()).await;
    let over = request(
        &api,
        &b.token,
        "POST",
        &second_path,
        Some(payload(&api, None, 1_000_001).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(over["bank_rsp_code"], "408");
    assert_eq!(
        movements(&api, second["loan_id"].as_str().unwrap().parse().unwrap()).await,
        no_movement
    );
    let insufficient = payload(&api, None, 1).await;
    let rejection_rid = new_id();
    let (status, rejected) = call(
        &api.public,
        "POST",
        &second_path,
        &rejection_rid,
        Some(&b.token),
        Some(insufficient.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_contract("BankRejected", &rejected);
    assert_eq!(rejected["bank_rsp_code"], "405");
    transfer(&api, &a, &b, 10).await;
    let replay = request(
        &api,
        &b.token,
        "POST",
        &second_path,
        Some(insufficient),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(core(rejected), core(replay));
    assert_eq!(snapshot(&api, &b.fin).await, (before_b.0 + 10, before_b.1));
    completion(&api, rejection_rid, "LOAN_REPAY").await;
    assert_eq!(snapshot(&api, second_fin).await, before_loan);
    for (path, fin) in [(ROOT.to_owned(), Some(a.fin.as_str())), (repay_path, None)] {
        let valid = payload(&api, fin, 1_000_000).await;
        let mut bad_id = valid.clone();
        bad_id["bank_tran_id"] = json!("bad");
        let mut zero = valid;
        zero["tran_amt"] = json!("0");
        for bad in [bad_id, zero, json!({})] {
            let response = request(
                &api,
                &a.token,
                "POST",
                &path,
                Some(bad),
                StatusCode::BAD_REQUEST,
                "Error",
            )
            .await;
            assert_eq!(response["rsp_code"], "A0002");
        }
    }
}

#[tokio::test]
async fn account_guards() {
    let api = api().await;
    let a = customer(&api, 200_000).await;
    let deposit = request(
        &api,
        &a.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(common::deposits::open_payload(&api, &a.fin, 100_000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let response = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload(&api, deposit["fintech_use_num"].as_str(), 1_000_000).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(response["bank_rsp_code"], "403");
    let loan = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&a.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    let id = loan["loan_id"].as_str().unwrap().parse().unwrap();
    let before = movements(&api, id).await;
    close_demand(&api, &a.fin).await;
    let response = request(
        &api,
        &a.token,
        "POST",
        &format!("{ROOT}/{id}/repay"),
        Some(payload(&api, None, 1).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(response["bank_rsp_code"], "403");
    assert_eq!(movements(&api, id).await, before);
    assert_eq!(
        snapshot(&api, loan["fintech_use_num"].as_str().unwrap())
            .await
            .0,
        1_000_000
    );
    let b = customer(&api, 0).await;
    close_demand(&api, &b.fin).await;
    let response = request(
        &api,
        &b.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&b.fin), 1_000_000).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(response["bank_rsp_code"], "403");
    assert_eq!(snapshot(&api, &b.fin).await.0, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_loans() {
    let api = std::sync::Arc::new(api().await);
    let a = customer(&api, 0).await;
    let body = payload(&api, Some(&a.fin), 1_000_000).await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let api = api.clone();
        let token = a.token.clone();
        let body = body.clone();
        tasks.push(tokio::spawn(async move {
            call(
                &api.public,
                "POST",
                ROOT,
                &new_id(),
                Some(&token),
                Some(body),
            )
            .await
        }));
    }
    let mut success = None;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        if status == StatusCode::OK {
            assert_contract("LoanTranResponse", &body);
            if let Some(previous) = &success {
                assert_eq!(&core(body.clone()), previous);
            }
            success = Some(core(body));
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert_contract("Error", &body);
        }
    }
    let loan = success.expect("at least one take succeeded");
    let id = loan["loan_id"].as_str().unwrap().parse::<i64>().unwrap();
    assert_eq!(
        movements(&api, id).await,
        vec![("loan_disburse".into(), 2, 1_000_000, 1_000_000, 4)]
    );
    let sql = bank::sql_stmt!("test.loan_take_count", "sql/loan_take_count.sql");
    let fin = a.fin.clone();
    let counts: (i64, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&sql, &[&fin]))
        .await
        .unwrap();
    assert_eq!(counts, (1, 1));
    assert_eq!(snapshot(&api, &a.fin).await.0, 1_000_000);

    let second = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&a.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    // Leave less cash than the aggregate ten repayments, with neither loan individually overdrawn.
    request(
        &api,
        &a.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(common::deposits::open_payload(&api, &a.fin, 1_500_000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let starting = snapshot(&api, &a.fin).await.0;
    let loans = [loan, second];
    let mut tasks = Vec::new();
    for i in 0..10 {
        let api = api.clone();
        let token = a.token.clone();
        let index = i % 2;
        let path = format!("{ROOT}/{}/repay", loans[index]["loan_id"].as_str().unwrap());
        let body = payload(&api, None, 100_000).await;
        tasks.push(tokio::spawn(async move {
            (
                index,
                call(
                    &api.public,
                    "POST",
                    &path,
                    &new_id(),
                    Some(&token),
                    Some(body),
                )
                .await,
            )
        }));
    }
    let mut paid = [0, 0];
    for task in tasks {
        let (index, (status, body)) = task.await.unwrap();
        check_repayment(status, &body);
        if status == StatusCode::OK {
            paid[index] += 100_000;
        }
    }
    let balance = snapshot(&api, &a.fin).await.0;
    assert!(balance >= 0);
    assert_eq!(balance + paid.iter().sum::<i64>(), starting);
    for (index, loan) in loans.iter().enumerate() {
        assert_eq!(
            snapshot(&api, loan["fintech_use_num"].as_str().unwrap())
                .await
                .0,
            1_000_000 - paid[index]
        );
    }

    // All requests race for the same final principal; at most one closes the account.
    let c = customer(&api, 0).await;
    let loan = request(
        &api,
        &c.token,
        "POST",
        ROOT,
        Some(payload(&api, Some(&c.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    let id = loan["loan_id"].as_str().unwrap().parse::<i64>().unwrap();
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let api = api.clone();
        let token = c.token.clone();
        let body = payload(&api, None, 250_000).await;
        let path = format!("{ROOT}/{id}/repay");
        tasks.push(tokio::spawn(async move {
            call(
                &api.public,
                "POST",
                &path,
                &new_id(),
                Some(&token),
                Some(body),
            )
            .await
        }));
    }
    let mut paid = 0;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        if status == StatusCode::UNPROCESSABLE_ENTITY && body["bank_rsp_code"] == "403" {
            // A serialized contender can observe the account after the winning close.
            assert_contract("BankRejected", &body);
        } else {
            check_repayment(status, &body);
        }
        if status == StatusCode::OK {
            paid += 250_000;
        }
    }
    let owed = snapshot(&api, loan["fintech_use_num"].as_str().unwrap())
        .await
        .0;
    assert!(owed >= 0);
    assert_eq!(owed, 1_000_000 - paid);
    assert_eq!(snapshot(&api, &c.fin).await.0 + paid, 1_000_000);
    let sql = bank::sql_stmt!("test.loan_closed_count", "sql/loan_closed_count.sql");
    let (status, count): (String, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&sql, &[&id]))
        .await
        .unwrap();
    assert!(count <= 1);
    assert_eq!(count, i64::from(owed == 0));
    assert_eq!(status, if owed == 0 { "CLOSED" } else { "ACTIVE" });
}

fn check_repayment(status: StatusCode, body: &serde_json::Value) {
    match status {
        StatusCode::OK => assert_contract("LoanTranResponse", body),
        StatusCode::UNPROCESSABLE_ENTITY => {
            assert_contract("BankRejected", body);
            assert!(
                matches!(body["bank_rsp_code"].as_str(), Some("405" | "408")),
                "{body}"
            );
        }
        StatusCode::SERVICE_UNAVAILABLE => assert_contract("Error", body),
        _ => panic!("unexpected {status}: {body}"),
    }
}
