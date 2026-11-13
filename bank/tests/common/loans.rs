pub use crate::common::deposits::{core, request};
use crate::common::*;
use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use serde_json::{Value, json};
const SNAPSHOT: SqlStmt = sql_stmt!("test.loan_snapshot", "../sql/loan_snapshot.sql");
const MOVEMENTS: SqlStmt = sql_stmt!("test.loan_movements", "../sql/loan_movements.sql");
const REQUESTS: SqlStmt = sql_stmt!("test.loan_requests", "../sql/loan_requests.sql");
pub const CLOSURE: SqlStmt = sql_stmt!("test.loan_closure", "../sql/loan_closure.sql");
pub async fn payload(api: &Api, fin: Option<&str>, amount: i64) -> Value {
    let mut v = json!({"bank_tran_id": bank_tran_id(), "tran_amt": amount.to_string(),
        "tran_dtime": api.state.clock.state().await.unwrap().business_time.format("%Y%m%d%H%M%S").to_string()});
    if let Some(fin) = fin {
        v["fintech_use_num"] = json!(fin);
        v["terms_id"] = json!("5");
    }
    v
}
pub async fn snapshot(api: &Api, fin: &str) -> (i64, i64) {
    let fin = fin.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&SNAPSHOT, &[&fin]))
        .await
        .unwrap()
}
pub async fn movements(api: &Api, id: i64) -> Vec<(String, i64, i64, i64, i64)> {
    api.state
        .db
        .run(move |s| s.query_all(&MOVEMENTS, &[&id, &Option::<String>::None]))
        .await
        .unwrap()
}
pub async fn completion(api: &Api, rid: String, operation: &str) {
    let row: (String, String, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&REQUESTS, &[&rid]))
        .await
        .unwrap();
    assert_eq!(row, (operation.into(), "MOBILE".into(), 1));
}

pub async fn provenance(api: &Api, id: i64, rid: &str, kind: &str, amount: i64) {
    let rid = rid.to_owned();
    let rows: Vec<(String, i64, i64, i64, i64)> = api
        .state
        .db
        .run(move |s| s.query_all(&MOVEMENTS, &[&id, &rid]))
        .await
        .unwrap();
    assert_eq!(rows, vec![(kind.into(), 2, amount, amount, 4)]);
}
pub async fn transfer(api: &Api, from: &Customer, to: &Customer, amount: i64) {
    let body = json!({"bank_tran_id": bank_tran_id(), "fintech_use_num": from.fin,
        "tran_amt": amount.to_string(), "transfer_purpose": "TR", "recv_client_name": "김수취",
        "recv_client_bank_code": "998", "recv_client_account_num": to.account,
        "tran_dtime": api.state.clock.state().await.unwrap().business_time.format("%Y%m%d%H%M%S").to_string()});
    let (status, body) = call(
        &api.public,
        "POST",
        "/v2.0/transfer/withdraw/fin_num",
        &new_id(),
        Some(&from.token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
pub async fn close_demand(api: &Api, fin: &str) {
    let fin = fin.to_owned();
    let sql = sql_stmt!("test.loan_close_demand", "../sql/loan_close_demand.sql");
    api.state
        .db
        .run(move |s| {
            assert_eq!(s.execute(&sql, &[&fin])?, 1);
            s.commit()
        })
        .await
        .unwrap();
}
