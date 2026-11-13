use super::*;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use serde_json::json;
pub const SNAPSHOT: SqlStmt = sql_stmt!("test.deposit_snapshot", "../sql/deposit_snapshot.sql");
pub const MOVEMENTS: SqlStmt = sql_stmt!("test.deposit_movements", "../sql/deposit_movements.sql");
pub const CLOSURE: SqlStmt = sql_stmt!("test.deposit_closure", "../sql/deposit_closure.sql");
pub const ROLLOVER: SqlStmt = sql_stmt!("test.deposit_rollover", "../sql/deposit_rollover.sql");
pub type MovementRow = (String, i64, i64, String, String, i64);
pub async fn request(
    api: &Api,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
    schema: &str,
) -> Value {
    let (status, body) = call(&api.public, method, path, &new_id(), Some(token), body).await;
    assert_eq!(status, expected, "{body}");
    assert_contract(schema, &body);
    body
}
pub async fn open_payload(api: &Api, fin: &str, amount: i64) -> Value {
    json!({"bank_tran_id": bank_tran_id(), "fintech_use_num": fin, "terms_id": "2", "tran_amt": amount.to_string(),
        "tran_dtime": api.state.clock.state().await.unwrap().business_time.format("%Y%m%d%H%M%S").to_string()})
}
pub async fn close_payload(api: &Api) -> Value {
    json!({"bank_tran_id": bank_tran_id(), "tran_dtime": api.state.clock.state().await.unwrap().business_time.format("%Y%m%d%H%M%S").to_string()})
}
pub async fn snapshot(api: &Api, fin: &str) -> (i64, i64) {
    let fin = fin.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&SNAPSHOT, &[&fin]))
        .await
        .unwrap()
}
pub async fn movements(api: &Api, id: i64) -> Vec<MovementRow> {
    api.state
        .db
        .run(move |s| s.query_all(&MOVEMENTS, &[&id]))
        .await
        .unwrap()
}
pub fn core(mut value: Value) -> Value {
    for key in ["request_id", "api_tran_id", "api_tran_dtm"] {
        value.as_object_mut().unwrap().remove(key);
    }
    value
}

pub async fn counterparties(api: &Api, id: i64, expected_rows: usize) {
    let sql = sql_stmt!(
        "test.product_counterparties",
        "../sql/product_counterparties.sql"
    );
    let rows: Vec<(String, String, String, String, String)> = api
        .state
        .db
        .run(move |s| s.query_all(&sql, &[&id]))
        .await
        .unwrap();
    assert_eq!(rows.len(), expected_rows);
    for (bank, account, name, other_account, holder) in rows {
        assert_eq!(bank, bank::shared::api::fields::BANK_CODE);
        assert_eq!(account, other_account);
        assert_eq!(name, holder);
        assert_eq!(holder, "김수취");
    }
}
