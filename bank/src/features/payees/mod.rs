//! Recipient inquiry and the authenticated customer's active payees.
use crate::app::AppState;
use crate::shared::api::fields::{BANK_CODE, BANK_NAME};
use crate::shared::api::{self, ApiError, codes, fields};
use crate::shared::auth::AuthCustomer;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Router};
use serde::Deserialize;
use serde_json::{Value, json};
const SELECT_INTERNAL: SqlStmt = sql_stmt!("payees.select_internal", "sql/select_internal.sql");
const SELECT_EXTERNAL: SqlStmt = sql_stmt!("payees.select_external", "sql/select_external.sql");
const INSERT_PAYEE: SqlStmt = sql_stmt!("payees.insert_payee", "sql/insert_payee.sql");
const LIST_PAYEES: SqlStmt = sql_stmt!("payees.list_payees", "sql/list_payees.sql");
const SELECT_PAYEE: SqlStmt = sql_stmt!("payees.select_payee", "sql/select_payee.sql");
const DELETE_PAYEE: SqlStmt = sql_stmt!("payees.delete_payee", "sql/delete_payee.sql");
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/inquiry/receive", post(inquiry))
        .route("/v2.0/ext/payees", get(list).post(register).delete(delete))
}
fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::error!(request_id = %ctx.request_id, error = %e, "payee database error");
    if matches!(e, DbError::Busy) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}
fn invalid(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "요청 항목 오류",
    )
}
fn valid_recipient(bank: &str, account: &str) -> bool {
    bank.len() == 3
        && bank.bytes().all(|b| b.is_ascii_digit())
        && !account.is_empty()
        && account.len() <= 16
        && account.bytes().all(|b| b.is_ascii_digit())
}
// A rejection is returned separately from infrastructure failures.
fn resolve(
    s: &mut Session,
    bank: &str,
    account: &str,
) -> Result<Result<String, &'static str>, DbError> {
    if bank == BANK_CODE {
        // Only an open demand account can receive a transfer.
        let found: Option<(String, String, String)> = s.query_opt(&SELECT_INTERNAL, &[&account])?;
        Ok(match found {
            Some((_, status, subtype)) if status == "CLOSED" || subtype != "DEMAND" => Err("403"),
            Some((name, _, _)) => Ok(name),
            None => Err("404"),
        })
    } else {
        let found: Option<String> = s.query_opt(&SELECT_EXTERNAL, &[&bank, &account])?;
        Ok(found.ok_or("404"))
    }
}
fn rejection_body(code: &str) -> Value {
    json!({"bank_rsp_code": code, "bank_rsp_message": if code == "403" { "계좌 상태 오류" } else { "계좌 없음" }})
}
#[derive(Deserialize)]
struct Inquiry {
    bank_tran_id: String,
    bank_code_std: String,
    account_num: String,
    tran_amt: String,
    tran_dtime: String,
}
async fn inquiry(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    _who: AuthCustomer,
    crate::shared::api::Json(req): crate::shared::api::Json<Inquiry>,
) -> Result<Response, ApiError> {
    if !valid_recipient(&req.bank_code_std, &req.account_num)
        || !fields::valid_bank_tran_id(&req.bank_tran_id)
        || !fields::valid_tran_dtime(&req.tran_dtime)
        || req.tran_amt.is_empty()
        || req.tran_amt.len() > 12
        || !req.tran_amt.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(&ctx));
    }
    let bank = req.bank_code_std.clone();
    let account = req.account_num.clone();
    let found = state
        .db
        .run_in(&stats, move |s| resolve(s, &bank, &account))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let mut body = json!({"bank_tran_id": req.bank_tran_id, "bank_tran_date": fields::date(now),
        "bank_code_tran": BANK_CODE, "bank_name": BANK_NAME, "bank_rsp_code": "000", "bank_rsp_message": ""});
    match found {
        Ok(name) => {
            body.as_object_mut().unwrap().extend(json!({"bank_code_std": req.bank_code_std,
            "account_num_masked": fields::mask_account(&req.account_num), "account_holder_name": name,
            "tran_amt": req.tran_amt}).as_object().unwrap().clone());
            Ok(api::ok(&ctx, body))
        }
        Err(code) => {
            body.as_object_mut()
                .unwrap()
                .extend(rejection_body(code).as_object().unwrap().clone());
            Ok(api::respond(
                &ctx,
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "참가기관 거부",
                body,
            ))
        }
    }
}
type PayeeRow = (i64, String, String, String, String, Option<String>, String);
fn payee(row: PayeeRow) -> Value {
    let (id, bank, bank_name, account, name, alias, time) = row;
    json!({"payee_id": id.to_string(), "bank_code_std": bank, "bank_name": bank_name,
        "account_num": account, "account_holder_name": name, "alias": alias.unwrap_or_default(), "registered_dtime": time})
}
#[derive(Deserialize)]
struct Create {
    bank_code_std: String,
    account_num: String,
    alias: Option<String>,
}
/// The public listener serves the mobile app; the channel is never taken from the request.
const CHANNEL: &str = "MOBILE";
async fn register(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Json(req): crate::shared::api::Json<Create>,
) -> Result<Response, ApiError> {
    if !valid_recipient(&req.bank_code_std, &req.account_num)
        || req.alias.as_ref().is_some_and(|a| a.chars().count() > 40)
    {
        return Err(invalid(&ctx));
    }
    let c = ctx.clone();
    let result = state.db.run_in(&stats, move |s| {
        let name = match resolve(s, &req.bank_code_std, &req.account_num)? { Ok(name) => name, Err(code) => return Ok(Err(code)) };
        let id = s.insert_returning_id(&INSERT_PAYEE, &[&who.customer_id, &req.bank_code_std, &req.account_num,
            &name, &req.alias, &now, &CHANNEL, &who.session_id])?;
        let row: PayeeRow = s.query_row(&SELECT_PAYEE, &[&id, &who.customer_id])?;
        outbox::record(s, &Event::for_request(&c, "payee.registered", now, CHANNEL, "completed")
            .customer(who.customer_no.clone()).session(who.session_id.clone()).codes(codes::SUCCESS, None)
            .payload(json!({"payee_id": id.to_string(), "bank_code_std": req.bank_code_std, "account_num": req.account_num})))?;
        outbox::record(s, &Event::for_request(&c, "request.completed", now, CHANNEL, "completed")
            .customer(who.customer_no).session(who.session_id).codes(codes::SUCCESS, None)
            .payload(json!({"http_status": 200})))?;
        s.commit()?; Ok(Ok(payee(row)))
    }).await.map_err(|e| {
        if e.ora_code() == Some(1) && e.to_string().to_uppercase().contains("PAYEE_ACTIVE_UK") {
            ApiError::new(&ctx, StatusCode::CONFLICT, codes::DUPLICATE_REQUEST, "이미 등록된 수취인입니다")
        } else { db_error(&ctx, &e) }
    })?;
    match result {
        Ok(body) => Ok(api::ok(&ctx, body)),
        Err(code) => Ok(api::respond(
            &ctx,
            StatusCode::UNPROCESSABLE_ENTITY,
            codes::BANK_REJECTED,
            "참가기관 거부",
            rejection_body(code),
        )),
    }
}
async fn list(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
) -> Result<Response, ApiError> {
    let rows: Vec<PayeeRow> = state
        .db
        .run_in(&stats, move |s| {
            s.query_all(&LIST_PAYEES, &[&who.customer_id])
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let list: Vec<Value> = rows.into_iter().map(payee).collect();
    Ok(api::ok(
        &ctx,
        json!({"res_cnt": list.len().to_string(), "res_list": list}),
    ))
}
#[derive(Deserialize)]
struct Delete {
    payee_id: i64,
}
async fn delete(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Query(req): crate::shared::api::Query<Delete>,
) -> Result<Response, ApiError> {
    let c = ctx.clone();
    let deleted = state.db.run_in(&stats, move |s| {
        let row: Option<PayeeRow> = s.query_opt(&SELECT_PAYEE, &[&req.payee_id, &who.customer_id])?;
        let Some(row) = row else { return Ok(false) };
        if s.execute(&DELETE_PAYEE, &[&now, &req.payee_id, &who.customer_id])? == 0 { return Ok(false) }
        outbox::record(s, &Event::for_request(&c, "payee.deleted", now, CHANNEL, "completed")
            .customer(who.customer_no.clone()).session(who.session_id.clone()).codes(codes::SUCCESS, None)
            .payload(json!({"payee_id": row.0.to_string(), "bank_code_std": row.1, "account_num": row.3})))?;
        outbox::record(s, &Event::for_request(&c, "request.completed", now, CHANNEL, "completed")
            .customer(who.customer_no).session(who.session_id).codes(codes::SUCCESS, None)
            .payload(json!({"http_status": 200})))?;
        s.commit()?; Ok(true)
    }).await.map_err(|e| db_error(&ctx, &e))?;
    if deleted {
        Ok(api::ok(&ctx, json!({})))
    } else {
        Err(ApiError::new(
            &ctx,
            StatusCode::NOT_FOUND,
            codes::NOT_FOUND,
            "수취인을 찾을 수 없습니다",
        ))
    }
}
