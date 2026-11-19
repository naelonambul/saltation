//! Read-only ledger, card, ATM and daily-close views for analysts.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use serde::Deserialize;
use serde_json::{Value, json};
use ulid::Ulid;

use super::{Page, PageInput, db_error, invalid, not_found, parse_row};
use crate::app::AppState;
use crate::shared::api::{self, ApiError};
use crate::shared::auth::AuthAnalyst;
use crate::shared::context::RequestContext;
use crate::shared::db::SqlStmt;
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const ACCOUNT_ID: SqlStmt = sql_stmt!("admin.view_account_id", "sql/view_account_id.sql");
const TRANSACTIONS: SqlStmt = sql_stmt!("admin.view_transactions", "sql/view_transactions.sql");
const TRANSFER: SqlStmt = sql_stmt!("admin.view_transfer", "sql/view_transfer.sql");
const TRANSFER_LEGS: SqlStmt = sql_stmt!("admin.view_transfer_legs", "sql/view_transfer_legs.sql");
const TERMINAL: SqlStmt = sql_stmt!("admin.view_terminal", "sql/view_terminal.sql");
const CARD_PAYMENTS: SqlStmt = sql_stmt!("admin.view_card_payments", "sql/view_card_payments.sql");
const DAILY_CLOSE: SqlStmt = sql_stmt!("admin.view_daily_close", "sql/view_daily_close.sql");

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/v1/accounts/{account_no}/transactions",
            get(transactions),
        )
        .route("/admin/v1/transfers/{transfer_id}", get(transfer))
        .route("/admin/v1/transactions/{tran_id}/terminal", get(terminal))
        .route(
            "/admin/v1/accounts/{account_no}/card-payments",
            get(card_payments),
        )
        .route("/admin/v1/daily-close", get(daily_close))
}

fn date(ctx: &RequestContext, raw: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .filter(|date| {
            (1..=9999).contains(&date.year()) && date.format("%Y-%m-%d").to_string() == raw
        })
        .ok_or_else(|| invalid(ctx))
}

fn numeric_id(ctx: &RequestContext, raw: &str) -> Result<i64, ApiError> {
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(ctx));
    }
    raw.parse().map_err(|_| invalid(ctx))
}

#[derive(Debug, Deserialize)]
struct TransactionQuery {
    from_date: Option<String>,
    to_date: Option<String>,
    channel: Option<String>,
    direction: Option<Direction>,
    min_amount: Option<u64>,
    max_amount: Option<u64>,
    limit: Option<u16>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Debit,
    Credit,
}

impl Direction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Debit => "debit",
            Self::Credit => "credit",
        }
    }
}

async fn transactions(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(account_no): Path<String>,
    query: Result<Query<TransactionQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|_| invalid(&ctx))?;
    let from_date = query
        .from_date
        .as_deref()
        .map(|s| date(&ctx, s))
        .transpose()?;
    let to_date = query
        .to_date
        .as_deref()
        .map(|s| date(&ctx, s))
        .transpose()?;
    let channel = query.channel;
    if channel.as_deref().is_some_and(|channel| {
        !matches!(channel, "MOBILE" | "ATM" | "CARD" | "EXTERNAL" | "SYSTEM")
    }) || matches!((from_date, to_date), (Some(from), Some(to)) if from > to)
        || matches!((query.min_amount, query.max_amount), (Some(min), Some(max)) if min > max)
    {
        return Err(invalid(&ctx));
    }

    let direction = query.direction.as_ref().map(Direction::as_str);
    let min_amount = query.min_amount;
    let max_amount = query.max_amount;
    let page = Page::new(
        &ctx,
        PageInput {
            limit: query.limit,
            cursor: query.cursor,
        },
        json!({
            "view": "transactions",
            "account_no": account_no,
            "from_date": from_date,
            "to_date": to_date,
            "channel": channel,
            "direction": direction,
            "min_amount": min_amount,
            "max_amount": max_amount,
        }),
    )?;
    let (cursor_date, cursor_sequence) = match page.cursor() {
        Some((day, sequence)) => (Some(date(&ctx, day)?), Some(numeric_id(&ctx, sequence)?)),
        None => (None, None),
    };
    // Oracle's final representable day needs no exclusive upper bound.
    let before_date = to_date
        .and_then(|date| date.succ_opt())
        .filter(|date| date.year() <= 9999);
    let take = page.take();
    let rows = state
        .db
        .run_in(&stats, move |session| {
            let Some(account_id) = session.query_opt::<i64>(&ACCOUNT_ID, &[&account_no])? else {
                return Ok(None);
            };
            session
                .query_all(
                    &TRANSACTIONS,
                    &[
                        &account_id,
                        &from_date,
                        &before_date,
                        &channel,
                        &direction,
                        &min_amount,
                        &max_amount,
                        &cursor_date,
                        &cursor_sequence,
                        &take,
                    ],
                )
                .map(Some)
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?
        .ok_or_else(|| not_found(&ctx))?;
    page.response(&ctx, rows)
}

async fn transfer(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(transfer_id): Path<String>,
) -> Result<Response, ApiError> {
    let transfer_id = numeric_id(&ctx, &transfer_id)?;
    let body = state
        .db
        .run_in(&stats, move |session| {
            let Some(row) = session.query_opt::<String>(&TRANSFER, &[&transfer_id])? else {
                return Ok(None);
            };
            let mut transfer = parse_row(&row)?;
            let legs = session
                .query_all::<String>(&TRANSFER_LEGS, &[&transfer_id])?
                .iter()
                .map(|row| parse_row(row))
                .collect::<Result<Vec<_>, _>>()?;
            transfer["legs"] = Value::Array(legs);
            Ok(Some(transfer))
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?
        .ok_or_else(|| not_found(&ctx))?;
    Ok(api::ok(&ctx, body))
}

async fn terminal(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(tran_id): Path<String>,
) -> Result<Response, ApiError> {
    let tran_id = numeric_id(&ctx, &tran_id)?;
    let row = state
        .db
        .run_in(&stats, move |session| {
            session.query_opt::<String>(&TERMINAL, &[&tran_id])
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?
        .ok_or_else(|| not_found(&ctx))?;
    let body = parse_row(&row).map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, body))
}

async fn card_payments(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(account_no): Path<String>,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(input) = query.map_err(|_| invalid(&ctx))?;
    let page = Page::new(
        &ctx,
        input,
        json!({"view": "card_payments", "account_no": account_no}),
    )?;
    let (cursor_time, cursor_request) = match page.cursor() {
        Some((time, request_id)) => {
            let format = "%Y-%m-%dT%H:%M:%S%.6f+09:00";
            let time = NaiveDateTime::parse_from_str(time, format)
                .ok()
                .filter(|parsed| {
                    (1..=9999).contains(&parsed.year())
                        && parsed.nanosecond() < 1_000_000_000
                        && parsed.format(format).to_string() == time
                })
                .ok_or_else(|| invalid(&ctx))?;
            let request = Ulid::from_string(request_id)
                .ok()
                .filter(|id| id.to_string() == request_id)
                .ok_or_else(|| invalid(&ctx))?;
            (Some(time), Some(request.to_string()))
        }
        None => (None, None),
    };
    let take = page.take();
    let rows = state
        .db
        .run_in(&stats, move |session| {
            let Some(account_id) = session.query_opt::<i64>(&ACCOUNT_ID, &[&account_no])? else {
                return Ok(None);
            };
            session
                .query_all(
                    &CARD_PAYMENTS,
                    &[&account_id, &cursor_time, &cursor_request, &take],
                )
                .map(Some)
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?
        .ok_or_else(|| not_found(&ctx))?;
    page.response(&ctx, rows)
}

#[derive(Debug, Deserialize)]
struct DailyCloseQuery {
    business_date: String,
    account_no: Option<String>,
    limit: Option<u16>,
    cursor: Option<String>,
}

async fn daily_close(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    query: Result<Query<DailyCloseQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|_| invalid(&ctx))?;
    let business_date = date(&ctx, &query.business_date)?;
    let account_no = query.account_no;
    let page = Page::new(
        &ctx,
        PageInput {
            limit: query.limit,
            cursor: query.cursor,
        },
        json!({"view": "daily_close", "business_date": business_date, "account_no": account_no}),
    )?;
    let after_account_id = match page.cursor() {
        Some((day, id)) if date(&ctx, day)? == business_date => Some(numeric_id(&ctx, id)?),
        Some(_) => return Err(invalid(&ctx)),
        None => None,
    };
    // Oracle binds an empty string as NULL. An explicit empty filter matches no account.
    if account_no.as_deref() == Some("") {
        return page.response(&ctx, Vec::new());
    }
    let take = page.take();
    let rows = state
        .db
        .run_in(&stats, move |session| {
            session.query_all(
                &DAILY_CLOSE,
                &[&business_date, &account_no, &after_account_id, &take],
            )
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    page.response(&ctx, rows)
}
