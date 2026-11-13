//! Open Banking inquiries (spec: Public interface): user info, account list,
//! balance and transaction history. Numbers are JSON strings, as in KFTC v3.3.6.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::shared::api::fields::{self, BANK_CODE, BANK_NAME};
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth::{self, AuthCustomer};
use crate::shared::clock::kst;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const SELECT_USER: SqlStmt = sql_stmt!("accounts.select_user", "sql/select_user.sql");
const SELECT_ACCOUNTS: SqlStmt = sql_stmt!("accounts.select_accounts", "sql/select_accounts.sql");
const SELECT_BALANCE: SqlStmt = sql_stmt!("accounts.select_balance", "sql/select_balance.sql");
const SELECT_DESC: SqlStmt = sql_stmt!(
    "accounts.select_transactions_desc",
    "sql/select_transactions_desc.sql"
);
const SELECT_ASC: SqlStmt = sql_stmt!(
    "accounts.select_transactions_asc",
    "sql/select_transactions_asc.sql"
);

/// KFTC `page_record_cnt` maximum.
const PAGE_SIZE: i64 = 25;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/user/me", get(user_me))
        .route("/v2.0/account/list", get(account_list))
        .route("/v2.0/account/balance/fin_num", get(balance))
        .route(
            "/v2.0/account/transaction_list/fin_num",
            get(transaction_list),
        )
}

fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::error!(request_id = %ctx.request_id, error = %e, "accounts database error");
    if matches!(e, DbError::Busy) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}

fn invalid(ctx: &RequestContext, message: &str) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        message,
    )
}

fn require_scope(ctx: &RequestContext, who: &AuthCustomer, scope: &str) -> Result<(), ApiError> {
    if who.has_scope(scope) {
        Ok(())
    } else {
        Err(ApiError::new(
            ctx,
            StatusCode::FORBIDDEN,
            codes::UNAUTHORIZED,
            "권한이 없습니다",
        ))
    }
}

type AccountRow = (String, String, String, String, String, i64);

fn account_json(name: &str, (fin, account_no, subtype, status, product, id): &AccountRow) -> Value {
    let mut account = json!({
        "fintech_use_num": fin,
        "account_alias": product,
        "bank_code_std": BANK_CODE,
        "bank_name": BANK_NAME,
        "account_num_masked": fields::mask_account(account_no),
        "account_holder_name": name,
        "account_type": fields::account_type(subtype),
        "inquiry_agree_yn": "Y",
        "transfer_agree_yn": if subtype == "DEMAND" { "Y" } else { "N" },
        "account_state": if status == "CLOSED" { "09" } else { "01" },
    });
    if matches!(subtype.as_str(), "TERM" | "LOAN") {
        account["ext_account_id"] = json!(id.to_string());
    }
    account
}

async fn load_accounts(
    state: &AppState,
    stats: &RequestStats,
    customer_id: i64,
) -> Result<(String, Vec<AccountRow>), DbError> {
    state
        .db
        .run_in(stats, move |s| {
            let name: String = s.query_row(&SELECT_USER, &[&customer_id])?;
            let rows: Vec<AccountRow> = s.query_all(&SELECT_ACCOUNTS, &[&customer_id])?;
            Ok((name, rows))
        })
        .await
}

async fn user_me(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "login")?;
    let (name, rows) = load_accounts(&state, &stats, who.customer_id)
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let list: Vec<Value> = rows.iter().map(|r| account_json(&name, r)).collect();
    Ok(api::ok(
        &ctx,
        json!({
            "user_seq_no": auth::user_seq_no(who.customer_id),
            "user_name": name,
            "res_cnt": list.len().to_string(),
            "res_list": list,
        }),
    ))
}

async fn account_list(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "inquiry")?;
    let (name, rows) = load_accounts(&state, &stats, who.customer_id)
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let list: Vec<Value> = rows.iter().map(|r| account_json(&name, r)).collect();
    Ok(api::ok(
        &ctx,
        json!({ "user_name": name, "res_cnt": list.len().to_string(), "res_list": list }),
    ))
}

#[derive(Debug, Deserialize)]
struct BalanceQuery {
    bank_tran_id: String,
    fintech_use_num: String,
    tran_dtime: String,
}

type BalanceRow = (
    i64,
    String,
    i64,
    String,
    String,
    DateTime<FixedOffset>,
    Option<String>,
    Option<String>,
);

fn not_found(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::NOT_FOUND,
        codes::NOT_FOUND,
        "계좌를 찾을 수 없습니다",
    )
}

fn bank_part(bank_tran_id: &str, now: DateTime<FixedOffset>) -> Value {
    json!({
        "bank_tran_id": bank_tran_id,
        "bank_tran_date": fields::date(now),
        "bank_code_tran": BANK_CODE,
        "bank_rsp_code": "000",
        "bank_rsp_message": "",
        "bank_name": BANK_NAME,
    })
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

async fn balance(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Query(q): crate::shared::api::Query<BalanceQuery>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "inquiry")?;
    if !fields::valid_bank_tran_id(&q.bank_tran_id) || !fields::valid_tran_dtime(&q.tran_dtime) {
        return Err(invalid(
            &ctx,
            "bank_tran_id 또는 tran_dtime 형식이 올바르지 않습니다",
        ));
    }

    let fin = q.fintech_use_num.clone();
    let customer_id = who.customer_id;
    let row: Option<BalanceRow> = state
        .db
        .run_in(&stats, move |s| {
            s.query_opt(&SELECT_BALANCE, &[&fin, &customer_id])
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let (_, _, balance, subtype, product, opened, last, maturity) =
        row.ok_or_else(|| not_found(&ctx))?;
    Ok(api::ok(
        &ctx,
        merge(
            bank_part(&q.bank_tran_id, now),
            json!({
                "fintech_use_num": q.fintech_use_num,
                "balance_amt": balance.to_string(),
                "available_amt": if subtype == "DEMAND" { balance.to_string() } else { "0".to_owned() },
                "account_type": fields::account_type(&subtype),
                "product_name": product,
                "account_issue_date": fields::date(opened),
                "maturity_date": maturity.unwrap_or_default(),
                "last_tran_date": last.unwrap_or_default(),
            }),
        ),
    ))
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    bank_tran_id: String,
    fintech_use_num: String,
    inquiry_type: String,
    inquiry_base: String,
    from_date: String,
    from_time: Option<String>,
    to_date: String,
    to_time: Option<String>,
    sort_order: String,
    tran_dtime: String,
    befor_inquiry_trace_info: Option<String>,
}

type HistoryRow = (
    i64,
    DateTime<FixedOffset>,
    i64,
    i64,
    String,
    String,
    Option<String>,
);

fn parse_bound(date: &str, time: Option<&str>, end: bool) -> Option<DateTime<FixedOffset>> {
    let d = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?;
    let t = match time {
        Some(t) if !t.is_empty() => NaiveTime::parse_from_str(t, "%H%M%S").ok()?,
        _ if end => NaiveTime::from_hms_opt(23, 59, 59)?,
        _ => NaiveTime::MIN,
    };
    let local = kst().from_local_datetime(&d.and_time(t)).single()?;
    // The upper bound is exclusive; include the whole final second.
    Some(if end {
        local + chrono::Duration::seconds(1)
    } else {
        local
    })
}

async fn transaction_list(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Query(q): crate::shared::api::Query<HistoryQuery>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "inquiry")?;
    if !fields::valid_bank_tran_id(&q.bank_tran_id) || !fields::valid_tran_dtime(&q.tran_dtime) {
        return Err(invalid(
            &ctx,
            "bank_tran_id 또는 tran_dtime 형식이 올바르지 않습니다",
        ));
    }
    if !matches!(q.inquiry_type.as_str(), "A" | "I" | "O")
        || !matches!(q.inquiry_base.as_str(), "D" | "T")
        || !matches!(q.sort_order.as_str(), "D" | "A")
    {
        return Err(invalid(
            &ctx,
            "inquiry_type, inquiry_base 또는 sort_order 값이 올바르지 않습니다",
        ));
    }
    let by_time = q.inquiry_base == "T";
    let from = parse_bound(
        &q.from_date,
        by_time.then_some(q.from_time.as_deref()).flatten(),
        false,
    );
    let to = parse_bound(
        &q.to_date,
        by_time.then_some(q.to_time.as_deref()).flatten(),
        true,
    );
    let (Some(from), Some(to)) = (from, to) else {
        return Err(invalid(&ctx, "조회 기간 형식이 올바르지 않습니다"));
    };
    if from >= to {
        return Err(invalid(&ctx, "조회 시작이 종료보다 늦습니다"));
    }
    let cursor = match q
        .befor_inquiry_trace_info
        .as_deref()
        .filter(|v| !v.is_empty())
    {
        Some(v) => Some(
            v.parse::<i64>()
                .map_err(|_| invalid(&ctx, "befor_inquiry_trace_info가 올바르지 않습니다"))?,
        ),
        None => None,
    };

    let fin = q.fintech_use_num.clone();
    let customer_id = who.customer_id;
    let itype = q.inquiry_type.clone();
    let descending = q.sort_order == "D";
    let result: Option<(i64, Vec<HistoryRow>)> = state
        .db
        .run_in(&stats, move |s| {
            let Some(row): Option<BalanceRow> =
                s.query_opt(&SELECT_BALANCE, &[&fin, &customer_id])?
            else {
                return Ok(None);
            };
            let (account_id, _, balance, ..) = row;
            let fetch = PAGE_SIZE + 1;
            let rows: Vec<HistoryRow> = if descending {
                let before = cursor.unwrap_or(i64::MAX);
                s.query_all(
                    &SELECT_DESC,
                    &[
                        &account_id,
                        &from,
                        &to,
                        &itype,
                        &itype,
                        &itype,
                        &before,
                        &fetch,
                    ],
                )?
            } else {
                let after = cursor.unwrap_or(0);
                s.query_all(
                    &SELECT_ASC,
                    &[
                        &account_id,
                        &from,
                        &to,
                        &itype,
                        &itype,
                        &itype,
                        &after,
                        &fetch,
                    ],
                )?
            };
            Ok(Some((balance, rows)))
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let (balance, mut rows) = result.ok_or_else(|| not_found(&ctx))?;
    let more = rows.len() as i64 > PAGE_SIZE;
    rows.truncate(PAGE_SIZE as usize);
    let trace = if more {
        rows.last().map(|r| r.0.to_string()).unwrap_or_default()
    } else {
        String::new()
    };
    let list: Vec<Value> = rows
        .iter()
        .map(|(_, at, amount, after, inout, tran_type, print)| {
            json!({
                "tran_date": fields::date(*at),
                "tran_time": fields::time(*at),
                "inout_type": inout,
                "tran_type": tran_type,
                "print_content": print.clone().unwrap_or_default(),
                "tran_amt": amount.abs().to_string(),
                "after_balance_amt": after.to_string(),
                "branch_name": BANK_NAME,
            })
        })
        .collect();
    Ok(api::ok(
        &ctx,
        merge(
            bank_part(&q.bank_tran_id, now),
            json!({
                "savings_bank_name": "",
                "fintech_use_num": q.fintech_use_num,
                "balance_amt": balance.to_string(),
                "page_record_cnt": list.len().to_string(),
                "next_page_yn": if more { "Y" } else { "N" },
                "befor_inquiry_trace_info": trace,
                "res_list": list,
            }),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_bounds_cover_whole_days_in_kst() {
        let from = parse_bound("20260103", None, false).expect("from");
        let to = parse_bound("20260103", None, true).expect("to");
        assert_eq!(from.to_rfc3339(), "2026-01-03T00:00:00+09:00");
        assert_eq!(to.to_rfc3339(), "2026-01-04T00:00:00+09:00");
        let t = parse_bound("20260103", Some("091244"), false).expect("time");
        assert_eq!(t.to_rfc3339(), "2026-01-03T09:12:44+09:00");
    }
}
