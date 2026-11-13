//! Internal interface for the generator (spec: Internal interface). Mounted
//! only on the generator listener. Stage 1: clock read, customer and demand
//! account seeding.

use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::fields::BANK_CODE;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth;
use crate::shared::clock::ClockError;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::ledger::{self, Kind, LedgerError, Leg, Movement};
use crate::shared::outbox;
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const INSERT_CUSTOMER: SqlStmt = sql_stmt!("internal.insert_customer", "sql/insert_customer.sql");
const SELECT_CUSTOMER_ID: SqlStmt =
    sql_stmt!("internal.select_customer_id", "sql/select_customer_id.sql");
const INSERT_CREDENTIAL: SqlStmt =
    sql_stmt!("internal.insert_credential", "sql/insert_credential.sql");
const INSERT_ACCOUNT: SqlStmt = sql_stmt!("internal.insert_account", "sql/insert_account.sql");
const INSERT_FINTECH: SqlStmt = sql_stmt!("internal.insert_fintech", "sql/insert_fintech.sql");
const INSERT_STATUS: SqlStmt = sql_stmt!(
    "internal.insert_status_history",
    "sql/insert_status_history.sql"
);

const MAX_BATCH: usize = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/v1/clock", get(read_clock))
        .route("/internal/v1/clock/set", post(set_clock))
        .route("/internal/v1/clock/advance", post(advance_clock))
        .route("/internal/v1/mode/live", post(go_live))
        .route("/internal/v1/outbox", get(read_outbox))
        .route("/internal/v1/seed/customers", post(seed_customers))
        .route("/internal/v1/seed/accounts", post(seed_accounts))
        .route(
            "/internal/v1/seed/counterparties",
            post(seed_counterparties),
        )
}

fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::error!(request_id = %ctx.request_id, error = %e, "internal database error");
    match e.ora_code() {
        Some(1) => ApiError::new(
            ctx,
            StatusCode::CONFLICT,
            codes::DUPLICATE_REQUEST,
            "이미 존재합니다",
        ),
        _ if matches!(e, DbError::Busy) => ApiError::busy(ctx),
        _ => ApiError::internal(ctx),
    }
}

fn bad_request(ctx: &RequestContext, message: &str) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        message,
    )
}

async fn read_clock(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<Response, ApiError> {
    let clock = state.clock.state().await.map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, clock))
}

/// Outbox backlog, used by the generator for backpressure (spec F-36a).
async fn read_outbox(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<Response, ApiError> {
    let (undelivered, oldest) = outbox::backlog(&state.db)
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(
        &ctx,
        json!({ "undelivered": undelivered, "oldest_age_seconds": oldest }),
    ))
}

#[derive(Debug, Deserialize)]
struct SeedCustomer {
    customer_no: String,
    name: String,
    phone: String,
    address: Option<String>,
    customer_type: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct SeedCustomers {
    customers: Vec<SeedCustomer>,
}

/// A synthetic connecting-information hash: never derived from real data.
fn ci_hash(c: &SeedCustomer) -> String {
    auth::hex(&Sha256::digest(
        format!("saltation-ci|{}|{}", c.customer_no, c.phone).as_bytes(),
    ))
}

async fn seed_customers(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Json(req): Json<SeedCustomers>,
) -> Result<Response, ApiError> {
    if req.customers.is_empty() || req.customers.len() > MAX_BATCH {
        return Err(bad_request(&ctx, "customers must contain 1 to 500 entries"));
    }

    if req.customers.iter().any(|c| c.name.chars().count() > 50) {
        return Err(bad_request(&ctx, "name must be at most 50 characters"));
    }
    let mut rows = Vec::with_capacity(req.customers.len());
    for c in req.customers {
        let hash = auth::hash_secret(c.password.clone())
            .await
            .map_err(|_| ApiError::internal(&ctx))?;
        rows.push((ci_hash(&c), hash, c));
    }
    let created = rows.len();
    state
        .db
        .run_in(&stats, move |s| {
            for (ci, hash, c) in &rows {
                let id = s.insert_returning_id(
                    &INSERT_CUSTOMER,
                    &[
                        &c.customer_no,
                        ci,
                        &c.name,
                        &c.phone,
                        &c.address,
                        &c.customer_type,
                        &now,
                    ],
                )?;
                s.execute(&INSERT_CREDENTIAL, &[&id, hash, &now])?;
            }
            s.commit()
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, json!({ "created": created })))
}

#[derive(Debug, Deserialize)]
struct SeedAccount {
    customer_no: String,
    account_no: String,
    /// Optional opening balance, posted as an inbound movement.
    #[serde(default)]
    opening_balance: i64,
}

#[derive(Debug, Deserialize)]
struct SeedAccounts {
    accounts: Vec<SeedAccount>,
}

/// Open Banking fintech_use_num, AN(24): bank code plus the zero-padded account ID.
fn fintech_use_num(account_id: i64) -> String {
    format!("{BANK_CODE}{account_id:021}")
}

async fn seed_accounts(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Json(req): Json<SeedAccounts>,
) -> Result<Response, ApiError> {
    if req.accounts.is_empty() || req.accounts.len() > MAX_BATCH {
        return Err(bad_request(&ctx, "accounts must contain 1 to 500 entries"));
    }
    if req
        .accounts
        .iter()
        .any(|a| a.account_no.len() > 16 || !a.account_no.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(bad_request(&ctx, "account_no must be up to 16 digits"));
    }

    let accounts = req.accounts;
    let created = state
        .db
        .run_in(&stats, move |s| {
            let mut out = Vec::with_capacity(accounts.len());
            for a in &accounts {
                let customer_id: i64 = s.query_row(&SELECT_CUSTOMER_ID, &[&a.customer_no])?;
                let account_id =
                    s.insert_returning_id(&INSERT_ACCOUNT, &[&a.account_no, &customer_id, &now])?;
                let fin = fintech_use_num(account_id);
                s.execute(
                    &INSERT_FINTECH,
                    &[&fin, &account_id, &auth::user_seq_no(customer_id)],
                )?;
                s.execute(&INSERT_STATUS, &[&account_id, &now])?;
                if a.opening_balance > 0 {
                    let mut locks = ledger::lock_accounts(s, &[account_id])?;
                    let movement = Movement {
                        kind: Kind::Inbound,
                        payee_registered: None,
                        request_id: None,
                        ctx: None,
                        bank_tran_id: None,
                        amount: a.opening_balance,
                        channel: "SYSTEM",
                        occurred_at: now,
                        legs: vec![Leg {
                            account_id,
                            customer_initiated: false,
                            amount: a.opening_balance,
                            inout_type: "입금",
                            tran_type: "대체",
                            print_content: Some("개설입금".into()),
                            counterparty_bank_code: None,
                            counterparty_account_no: None,
                            counterparty_name: Some("살테이션 초기화".into()),
                            transfer_out: 0,
                        }],
                    };
                    ledger::post(s, &mut locks, &movement).map_err(|e| match e {
                        LedgerError::Db(e) => e,
                        other => DbError::Config(other.to_string()),
                    })?;
                }
                out.push(json!({ "account_no": a.account_no, "fintech_use_num": fin }));
            }
            s.commit()?;
            Ok(out)
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, json!({ "accounts": created })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fintech_use_num_is_24_characters() {
        assert_eq!(fintech_use_num(7), "998000000000000000000007");
        assert_eq!(fintech_use_num(7).len(), 24);
    }
}

fn clock_error(ctx: &RequestContext, error: ClockError) -> ApiError {
    match error {
        ClockError::Busy => ApiError::busy(ctx),
        ClockError::Db(e) => db_error(ctx, &e),
        other => bad_request(ctx, &other.to_string()),
    }
}

#[derive(Deserialize)]
struct ClockSet {
    business_time: DateTime<FixedOffset>,
}

#[derive(Deserialize)]
struct ClockAdvance {
    seconds: i64,
}

async fn set_clock(
    State(state): State<AppState>,
    ctx: RequestContext,
    body: Result<Json<ClockSet>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|_| bad_request(&ctx, "invalid business_time"))?;
    let clock = state
        .clock
        .set(body.business_time)
        .await
        .map_err(|e| clock_error(&ctx, e))?;
    Ok(api::ok(&ctx, clock))
}

async fn advance_clock(
    State(state): State<AppState>,
    ctx: RequestContext,
    body: Result<Json<ClockAdvance>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|_| bad_request(&ctx, "invalid seconds"))?;
    if body.seconds < 1 {
        return Err(bad_request(&ctx, "seconds must be at least 1"));
    }
    let clock = state
        .clock
        .advance(body.seconds)
        .await
        .map_err(|e| clock_error(&ctx, e))?;
    Ok(api::ok(&ctx, clock))
}

async fn go_live(State(state): State<AppState>, ctx: RequestContext) -> Result<Response, ApiError> {
    let clock = state
        .clock
        .go_live()
        .await
        .map_err(|e| clock_error(&ctx, e))?;
    Ok(api::ok(&ctx, clock))
}

const INSERT_COUNTERPARTY: SqlStmt = sql_stmt!(
    "internal.insert_counterparty",
    "sql/insert_counterparty.sql"
);
#[derive(Deserialize)]
struct SeedCounterparty {
    counterparty_type: String,
    industry_code: Option<String>,
    institution_code: String,
    account_no: String,
    name: String,
}
#[derive(Deserialize)]
struct SeedCounterparties {
    counterparties: Vec<SeedCounterparty>,
}
async fn seed_counterparties(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Json(req): Json<SeedCounterparties>,
) -> Result<Response, ApiError> {
    if req.counterparties.is_empty()
        || req.counterparties.len() > MAX_BATCH
        || req.counterparties.iter().any(|c| {
            c.name.chars().count() > 50
                || !matches!(
                    c.counterparty_type.as_str(),
                    "EMPLOYER" | "MERCHANT" | "INDIVIDUAL"
                )
                || c.industry_code.as_ref().is_some_and(|v| {
                    v.is_empty() || v.len() > 10 || !v.bytes().all(|b| b.is_ascii_alphanumeric())
                })
                || c.institution_code.len() != 3
                || !c.institution_code.bytes().all(|b| b.is_ascii_digit())
                || c.account_no.is_empty()
                || c.account_no.len() > 16
                || !c.account_no.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return Err(bad_request(&ctx, "invalid counterparties"));
    }
    let created = state
        .db
        .run_in(&stats, move |s| {
            let mut created = 0u64;
            for c in req.counterparties {
                match s.execute(
                    &INSERT_COUNTERPARTY,
                    &[
                        &c.counterparty_type,
                        &c.industry_code,
                        &c.institution_code,
                        &c.account_no,
                        &c.name,
                        &now,
                    ],
                ) {
                    Ok(n) => created += n,
                    Err(e) if e.ora_code() == Some(1) => {}
                    Err(e) => return Err(e),
                }
            }
            s.commit()?;
            Ok(created)
        })
        .await
        .map_err(|e| {
            if e.ora_code() == Some(2291) {
                bad_request(&ctx, "unknown institution code")
            } else {
                db_error(&ctx, &e)
            }
        })?;
    Ok(api::ok(&ctx, json!({ "created": created })))
}
