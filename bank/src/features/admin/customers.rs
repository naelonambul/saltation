//! Analyst customer search and profile collections.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use serde::Deserialize;
use serde_json::json;

use super::{Page, PageInput, db_error, invalid, not_found, parse_row};
use crate::app::AppState;
use crate::shared::api::{self, ApiError};
use crate::shared::auth::AuthAnalyst;
use crate::shared::context::RequestContext;
use crate::shared::db::SqlStmt;
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const SEARCH_NAME: SqlStmt =
    sql_stmt!("admin.customer_search_name", "sql/customer_search_name.sql");
const SEARCH_EXACT: SqlStmt = sql_stmt!(
    "admin.customer_search_exact",
    "sql/customer_search_exact.sql"
);
const DETAIL: SqlStmt = sql_stmt!("admin.customer_detail", "sql/customer_detail.sql");
const PARENT: SqlStmt = sql_stmt!("admin.customer_parent", "sql/customer_parent.sql");
const ACCOUNTS: SqlStmt = sql_stmt!("admin.customer_accounts", "sql/customer_accounts.sql");
const CARDS: SqlStmt = sql_stmt!("admin.customer_cards", "sql/customer_cards.sql");
const DEPOSITS: SqlStmt = sql_stmt!("admin.customer_deposits", "sql/customer_deposits.sql");
const LOANS: SqlStmt = sql_stmt!("admin.customer_loans", "sql/customer_loans.sql");
const PAYEES: SqlStmt = sql_stmt!("admin.customer_payees", "sql/customer_payees.sql");
const DEVICES: SqlStmt = sql_stmt!("admin.customer_devices", "sql/customer_devices.sql");
const SESSIONS: SqlStmt = sql_stmt!("admin.customer_sessions", "sql/customer_sessions.sql");
const LOGIN_ATTEMPTS: SqlStmt = sql_stmt!(
    "admin.customer_login_attempts",
    "sql/customer_login_attempts.sql"
);

#[derive(Clone, Copy)]
enum Collection {
    ById(SqlStmt),
    Devices,
    Sessions,
    LoginAttempts,
}

pub(super) fn router() -> Router<AppState> {
    let mut router = Router::new()
        .route("/admin/v1/customers", get(search))
        .route("/admin/v1/customers/{customer_no}", get(detail));

    for (suffix, kind) in [
        ("accounts", Collection::ById(ACCOUNTS)),
        ("cards", Collection::ById(CARDS)),
        ("deposits", Collection::ById(DEPOSITS)),
        ("loans", Collection::ById(LOANS)),
        ("payees", Collection::ById(PAYEES)),
        ("devices", Collection::Devices),
        ("sessions", Collection::Sessions),
        ("login-attempts", Collection::LoginAttempts),
    ] {
        router = router.route(
            &format!("/admin/v1/customers/{{customer_no}}/{suffix}"),
            get(collection).layer(Extension((suffix, kind))),
        );
    }
    router
}

#[derive(Deserialize)]
struct SearchInput {
    name: Option<String>,
    phone: Option<String>,
    customer_no: Option<String>,
    limit: Option<u16>,
    cursor: Option<String>,
}

type PageRow = (String, String, String);

fn parse_id(raw: &str, digits: usize) -> Option<i64> {
    let id: i64 = raw.parse().ok()?;
    (raw.trim_start_matches('-').len() <= digits && id.to_string() == raw).then_some(id)
}

fn parse_time(raw: &str) -> Option<DateTime<FixedOffset>> {
    let time = DateTime::parse_from_rfc3339(raw).ok()?;
    (time.offset().local_minus_utc() == 9 * 3600
        && (1..=9999).contains(&time.year())
        && time.nanosecond() < 1_000_000_000
        && time.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string() == raw)
        .then_some(time)
}

fn prefix_pattern(prefix: &str) -> String {
    let mut pattern = String::with_capacity(prefix.len() + 1);
    for ch in prefix.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

async fn search(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    query: Result<Query<SearchInput>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|_| invalid(&ctx))?;
    let input = PageInput {
        limit: query.limit,
        cursor: query.cursor,
    };
    let (selector, term) = match (query.name, query.phone, query.customer_no) {
        (Some(term), None, None) => ("name", term),
        (None, Some(term), None) => ("phone", term),
        (None, None, Some(term)) => ("customer_no", term),
        _ => return Err(invalid(&ctx)),
    };
    if term.trim().is_empty() {
        return Err(invalid(&ctx));
    }

    let page = Page::new(&ctx, input, json!(["customers", selector, term]))?;
    let take = page.take();
    let rows: Vec<PageRow> = if selector == "name" {
        let (name, id) = match page.cursor() {
            Some((name, id)) if (1..=60).contains(&name.chars().count()) => (
                Some(name.to_owned()),
                Some(parse_id(id, 12).ok_or_else(|| invalid(&ctx))?),
            ),
            Some(_) => return Err(invalid(&ctx)),
            None => (None, None),
        };
        let pattern = prefix_pattern(&term);
        state
            .db
            .run_in(&stats, move |s| {
                s.query_all(&SEARCH_NAME, &[&pattern, &name, &id, &take])
            })
            .await
    } else {
        if page.cursor().is_some() {
            return Err(invalid(&ctx));
        }
        let (phone, no) = if selector == "phone" {
            (Some(term), None)
        } else {
            (None, Some(term))
        };
        state
            .db
            .run_in(&stats, move |s| s.query_all(&SEARCH_EXACT, &[&phone, &no]))
            .await
    }
    .map_err(|e| db_error(&ctx, &e))?;
    page.response(&ctx, rows)
}

async fn detail(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(customer_no): Path<String>,
) -> Result<Response, ApiError> {
    let row: Option<String> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&DETAIL, &[&customer_no]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let row = row.ok_or_else(|| not_found(&ctx))?;
    let value = parse_row(&row).map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, value))
}

enum Cursor {
    Id(SqlStmt, Option<i64>),
    Device(Option<String>),
    Session(Option<DateTime<FixedOffset>>, Option<String>),
    Attempt(Option<DateTime<FixedOffset>>, Option<i64>),
}

impl Collection {
    fn cursor(self, page: &Page, ctx: &RequestContext) -> Result<Cursor, ApiError> {
        let bad = || invalid(ctx);
        Ok(match (self, page.cursor()) {
            (Self::ById(stmt), None) => Cursor::Id(stmt, None),
            (Self::ById(stmt), Some((id, "-"))) => {
                Cursor::Id(stmt, Some(parse_id(id, 12).ok_or_else(bad)?))
            }
            (Self::Devices, None) => Cursor::Device(None),
            (Self::Devices, Some((id, "-"))) if (1..=64).contains(&id.chars().count()) => {
                Cursor::Device(Some(id.to_owned()))
            }
            (Self::Sessions, None) => Cursor::Session(None, None),
            (Self::Sessions, Some((time, id))) if id.parse::<ulid::Ulid>().is_ok() => {
                Cursor::Session(Some(parse_time(time).ok_or_else(bad)?), Some(id.to_owned()))
            }
            (Self::LoginAttempts, None) => Cursor::Attempt(None, None),
            (Self::LoginAttempts, Some((time, id))) => Cursor::Attempt(
                Some(parse_time(time).ok_or_else(bad)?),
                Some(parse_id(id, 15).ok_or_else(bad)?),
            ),
            _ => return Err(bad()),
        })
    }
}

async fn collection(
    _analyst: AuthAnalyst,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Extension((suffix, kind)): Extension<(&'static str, Collection)>,
    Path(customer_no): Path<String>,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(input) = query.map_err(|_| invalid(&ctx))?;
    let page = Page::new(&ctx, input, json!(["customers", customer_no, suffix]))?;
    let cursor = kind.cursor(&page, &ctx)?;
    let take = page.take();
    let rows: Option<Vec<PageRow>> = state
        .db
        .run_in(&stats, move |s| {
            let parent: Option<(i64, String)> = s.query_opt(&PARENT, &[&customer_no])?;
            let Some((id, phone)) = parent else {
                return Ok(None);
            };

            let rows = match cursor {
                Cursor::Id(stmt, after) => s.query_all(&stmt, &[&id, &after, &take]),
                Cursor::Device(after) => s.query_all(&DEVICES, &[&id, &after, &take]),
                Cursor::Session(time, after) => {
                    s.query_all(&SESSIONS, &[&id, &time, &after, &take])
                }
                Cursor::Attempt(time, after) => {
                    s.query_all(&LOGIN_ATTEMPTS, &[&id, &phone, &time, &after, &take])
                }
            }?;
            Ok(Some(rows))
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    page.response(&ctx, rows.ok_or_else(|| not_found(&ctx))?)
}
