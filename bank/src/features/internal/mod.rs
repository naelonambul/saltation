//! Internal interface for the generator (spec: Internal interface). Mounted
//! only on the generator listener. Stage 1: clock read, customer and demand
//! account seeding.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth;
use crate::shared::context::RequestContext;
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const INSERT_CUSTOMER: SqlStmt = sql_stmt!("internal.insert_customer", "sql/insert_customer.sql");
const SELECT_CUSTOMER_ID: SqlStmt =
    sql_stmt!("internal.select_customer_id", "sql/select_customer_id.sql");
const INSERT_CREDENTIAL: SqlStmt =
    sql_stmt!("internal.insert_credential", "sql/insert_credential.sql");
const INSERT_ACCOUNT: SqlStmt = sql_stmt!("internal.insert_account", "sql/insert_account.sql");
const SELECT_ACCOUNT_ID: SqlStmt =
    sql_stmt!("internal.select_account_id", "sql/select_account_id.sql");
const INSERT_FINTECH: SqlStmt = sql_stmt!("internal.insert_fintech", "sql/insert_fintech.sql");
const INSERT_STATUS: SqlStmt = sql_stmt!(
    "internal.insert_status_history",
    "sql/insert_status_history.sql"
);

const MAX_BATCH: usize = 500;
/// Saltation's synthetic bank code (contracts/codes).
pub const BANK_CODE: &str = "998";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/v1/clock", get(read_clock))
        .route("/internal/v1/seed/customers", post(seed_customers))
        .route("/internal/v1/seed/accounts", post(seed_accounts))
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
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Json(req): Json<SeedCustomers>,
) -> Result<Response, ApiError> {
    if req.customers.is_empty() || req.customers.len() > MAX_BATCH {
        return Err(bad_request(&ctx, "customers must contain 1 to 500 entries"));
    }
    let now = state.clock.now().await.map_err(|e| db_error(&ctx, &e))?;
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
                s.execute(
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
                let id: i64 = s.query_row(&SELECT_CUSTOMER_ID, &[&c.customer_no])?;
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
    let now = state.clock.now().await.map_err(|e| db_error(&ctx, &e))?;
    let accounts = req.accounts;
    let created = state
        .db
        .run_in(&stats, move |s| {
            let mut out = Vec::with_capacity(accounts.len());
            for a in &accounts {
                let customer_id: i64 = s.query_row(&SELECT_CUSTOMER_ID, &[&a.customer_no])?;
                s.execute(&INSERT_ACCOUNT, &[&a.account_no, &customer_id, &now])?;
                let account_id: i64 = s.query_row(&SELECT_ACCOUNT_ID, &[&a.account_no])?;
                let fin = fintech_use_num(account_id);
                s.execute(
                    &INSERT_FINTECH,
                    &[&fin, &account_id, &auth::user_seq_no(customer_id)],
                )?;
                s.execute(&INSERT_STATUS, &[&account_id, &now])?;
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
