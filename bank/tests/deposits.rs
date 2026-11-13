//! Contract and ledger proofs for customer term deposits; never advances time.
mod common;
use axum::http::StatusCode;
use bank::shared::clock::business_date;
use bank::sql_stmt;
use common::deposits::*;
use common::*;
use serde_json::json;
const ROOT: &str = "/v2.0/ext/deposits";
#[tokio::test]
async fn deposits() {
    let api = api().await;
    let a = customer(&api, 2_000_000).await;
    let b = customer(&api, 0).await;
    let products = request(
        &api,
        &a.token,
        "GET",
        "/v2.0/ext/products",
        None,
        StatusCode::OK,
        "ProductListResponse",
    )
    .await;
    let list = products["res_list"].as_array().unwrap();
    let term = list.iter().find(|p| p["product_type"] == "TERM").unwrap();
    assert_eq!(
        term["terms_list"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["term_months"].clone())
            .collect::<Vec<_>>(),
        vec![json!("6"), json!("12"), json!("24")]
    );
    assert!(list.iter().any(|p| p["product_type"] == "LOAN"));
    let before = snapshot(&api, &a.fin).await;
    let payload = open_payload(&api, &a.fin, 100_000).await;
    let opened = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload.clone()),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let id = opened["deposit_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let path = format!("{ROOT}/{id}");
    assert_eq!(snapshot(&api, &a.fin).await, (before.0 - 100_000, before.1));
    assert_eq!(
        snapshot(&api, opened["fintech_use_num"].as_str().unwrap())
            .await
            .0,
        100_000
    );
    let maturity = business_date(api.state.clock.state().await.unwrap().business_time)
        .checked_add_months(chrono::Months::new(6))
        .unwrap()
        .format("%Y%m%d")
        .to_string();
    assert_eq!(opened["maturity_date"], maturity);
    let read = request(
        &api,
        &a.token,
        "GET",
        &path,
        None,
        StatusCode::OK,
        "DepositResponse",
    )
    .await;
    assert_eq!(read["fintech_use_num"], opened["fintech_use_num"]);
    assert_eq!(read["balance_amt"], "100000");
    let rows = movements(&api, id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (&rows[0].0, rows[0].1, rows[0].2),
        (&"term_open".to_owned(), 2, 0)
    );
    let replay = request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(payload.clone()),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    assert_eq!(core(replay), core(opened));
    assert_eq!(movements(&api, id).await, rows);
    assert_eq!(snapshot(&api, &a.fin).await, (before.0 - 100_000, before.1));
    let mut conflict = payload;
    conflict["tran_amt"] = json!("100001");
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
    for (amount, code) in [(99_999, "407"), (3_000_000, "405")] {
        let payload = open_payload(&api, &a.fin, amount).await;
        let rejected = request(
            &api,
            &a.token,
            "POST",
            ROOT,
            Some(payload.clone()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BankRejected",
        )
        .await;
        assert_eq!(rejected["bank_rsp_code"], code);
        let replay = request(
            &api,
            &a.token,
            "POST",
            ROOT,
            Some(payload),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BankRejected",
        )
        .await;
        assert_eq!(core(replay), core(rejected));
    }
    let foreign = open_payload(&api, &b.fin, 100_000).await;
    request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(foreign),
        StatusCode::NOT_FOUND,
        "Error",
    )
    .await;
    let mut loan = open_payload(&api, &a.fin, 100_000).await;
    loan["terms_id"] = json!("5");
    request(
        &api,
        &a.token,
        "POST",
        ROOT,
        Some(loan),
        StatusCode::NOT_FOUND,
        "Error",
    )
    .await;
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
    let close_path = format!("{path}/close");
    request(
        &api,
        &b.token,
        "POST",
        &close_path,
        Some(close_payload(&api).await),
        StatusCode::NOT_FOUND,
        "Error",
    )
    .await;
    let close_req = close_payload(&api).await;
    let closed = request(
        &api,
        &a.token,
        "POST",
        &close_path,
        Some(close_req.clone()),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    assert_eq!(closed["close_type"], "EARLY");
    assert_eq!(closed["account_state"], "09");
    assert_eq!(closed["balance_amt"], "0");
    assert_eq!(
        closed["close_date"],
        bank::shared::api::fields::date(api.state.clock.state().await.unwrap().business_time)
    );
    assert_eq!(snapshot(&api, &a.fin).await, before);
    let rows = movements(&api, id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (&rows[1].0, rows[1].1, rows[1].2),
        (&"term_close".to_owned(), 2, 0)
    );
    let closure: (String, i64, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&CLOSURE, &[&id]))
        .await
        .unwrap();
    assert_eq!(closure, ("EARLY".into(), 1, 1));
    let replay = request(
        &api,
        &a.token,
        "POST",
        &close_path,
        Some(close_req),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    assert_eq!(core(replay), core(closed));
    let rejected = request(
        &api,
        &a.token,
        "POST",
        &close_path,
        Some(close_payload(&api).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(rejected["bank_rsp_code"], "403");
    let read = request(
        &api,
        &a.token,
        "GET",
        &path,
        None,
        StatusCode::OK,
        "DepositResponse",
    )
    .await;
    assert_eq!(read["close_type"], "EARLY");
    assert_eq!(read["balance_amt"], "0");
    // Every successful or rejected money operation has one completion event.
    let events = sql_stmt!("test.deposit_requests", "sql/deposit_requests.sql");
    for (operation, payload, path, status, schema) in [
        (
            "TERM_OPEN",
            open_payload(&api, &a.fin, 100_000).await,
            ROOT.to_owned(),
            StatusCode::OK,
            "DepositTranResponse",
        ),
        (
            "TERM_OPEN",
            open_payload(&api, &a.fin, 1).await,
            ROOT.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BankRejected",
        ),
        (
            "TERM_CLOSE",
            close_payload(&api).await,
            close_path.clone(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BankRejected",
        ),
    ] {
        let rid = new_id();
        let (actual, response) = call(
            &api.public,
            "POST",
            &path,
            &rid,
            Some(&a.token),
            Some(payload),
        )
        .await;
        assert_eq!(actual, status);
        assert_contract(schema, &response);
        let meta: (String, String, i64) = api
            .state
            .db
            .run(move |s| s.query_row(&events, &[&rid]))
            .await
            .unwrap();
        assert_eq!(meta, (operation.into(), "MOBILE".into(), 1));
    }
    // Complete payloads with one bad field reach the handler's own validation.
    let mut bad_id = open_payload(&api, &a.fin, 100_000).await;
    bad_id["bank_tran_id"] = json!("bad");
    let mut zero = open_payload(&api, &a.fin, 100_000).await;
    zero["tran_amt"] = json!("0");
    let mut short_fin = open_payload(&api, &a.fin, 100_000).await;
    short_fin["fintech_use_num"] = json!("123");
    for mutation in [
        json!({}),
        json!({"bank_tran_id": "bad"}),
        bad_id,
        zero,
        short_fin,
    ] {
        request(
            &api,
            &a.token,
            "POST",
            ROOT,
            Some(mutation),
            StatusCode::BAD_REQUEST,
            "Error",
        )
        .await;
    }
    let bad = sql_stmt!("test.bad_transfers", "sql/bad_transfers.sql");
    let count: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&bad, &[]))
        .await
        .unwrap();
    assert_eq!(count, 0);
}
