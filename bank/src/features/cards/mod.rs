//! Synthetic debit card seeding and card network authorizations.
use std::io::{self, Read};
use std::sync::Arc;

use axum::{
    Extension, Router, extract::State, http::StatusCode, response::Response, routing::post,
};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

mod fx;
mod payment;

pub fn payment_router() -> Router<AppState> {
    Router::new().route("/card/v1/authorize", post(payment::authorize))
}

const PREFIX: &str = "0000";
const MAX_BATCH: usize = 500;
const ACCOUNT: SqlStmt = sql_stmt!("cards.account", "sql/account.sql");
const INSERT: SqlStmt = sql_stmt!("cards.insert", "sql/insert.sql");

fn luhn_sum(number: &str) -> u32 {
    number
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let n = u32::from(b - b'0');
            if i % 2 == 1 {
                let n = n * 2;
                if n > 9 { n - 9 } else { n }
            } else {
                n
            }
        })
        .sum()
}

fn valid_number(number: &str) -> bool {
    number.len() == 16
        && number.starts_with(PREFIX)
        && number.bytes().all(|b| b.is_ascii_digit())
        && luhn_sum(number).is_multiple_of(10)
}

/// Operating-system randomness, rejection-sampled to avoid biased decimal digits.
pub fn random_number() -> io::Result<String> {
    let mut source = std::fs::File::open("/dev/urandom")?;
    let mut number = PREFIX.to_owned();
    while number.len() < 15 {
        let mut byte = [0];
        source.read_exact(&mut byte)?;
        if byte[0] < 250 {
            number.push(char::from(b'0' + byte[0] % 10));
        }
    }
    number.push('0');
    let check = (10 - luhn_sum(&number) % 10) % 10;
    number.pop();
    number.push(char::from(b'0' + check as u8));
    Ok(number)
}

#[derive(Clone)]
struct Numbers(Arc<dyn Fn() -> io::Result<String> + Send + Sync>);

/// Number source is injected so uniqueness retries can be proved deterministically.
pub fn seed_router(
    numbers: impl Fn() -> io::Result<String> + Send + Sync + 'static,
) -> Router<AppState> {
    Router::new()
        .route("/internal/v1/seed/cards", post(seed))
        .layer(Extension(Numbers(Arc::new(numbers))))
}

#[derive(Deserialize)]
struct Card {
    account_no: String,
    status: String,
}
#[derive(Deserialize)]
struct Batch {
    cards: Vec<Card>,
}
fn invalid(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "invalid cards",
    )
}
async fn seed(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Extension(numbers): Extension<Numbers>,
    api::Json(req): api::Json<Batch>,
) -> Result<Response, ApiError> {
    if req.cards.is_empty()
        || req.cards.len() > MAX_BATCH
        || req.cards.iter().any(|c| {
            c.account_no.is_empty()
                || c.account_no.len() > 16
                || !c.account_no.bytes().all(|b| b.is_ascii_digit())
                || !matches!(c.status.as_str(), "ACTIVE" | "BLOCKED")
        })
    {
        return Err(invalid(&ctx));
    }
    let created = state.db.run_in(&stats, move |s| {
        let mut accounts = Vec::with_capacity(req.cards.len());
        for card in &req.cards {
            let id: Option<i64> = s.query_opt(&ACCOUNT, &[&card.account_no])?;
            let Some(id) = id else { return Ok(None); };
            accounts.push(id);
        }
        let mut created = Vec::with_capacity(req.cards.len());
        for (card, id) in req.cards.iter().zip(accounts) {
            let mut inserted = false;
            // One initial attempt plus five retries; the sixth collision fails the batch.
            for attempt in 0..=5 {
                let number = (numbers.0)().map_err(|e| DbError::Config(e.to_string()))?;
                if !valid_number(&number) { return Err(DbError::Config("invalid card number source".into())); }
                match s.execute(&INSERT, &[&number, &id, &card.status, &now]) {
                    Ok(_) => {
                        created.push(json!({"account_no": card.account_no, "card_no": number, "status": card.status}));
                        inserted = true;
                        break;
                    }
                    Err(e) if e.ora_code() == Some(1) && attempt < 5 => {}
                    Err(e) => return Err(e),
                }
            }
            debug_assert!(inserted);
        }
        s.commit()?;
        Ok(Some(created))
    }).await.map_err(|e| {
        tracing::error!(request_id = %ctx.request_id, error = %e, "card seed database error");
        match e.ora_code() {
            Some(1) => ApiError::new(&ctx, StatusCode::CONFLICT, codes::DUPLICATE_REQUEST, "이미 존재합니다"),
            _ if matches!(e, DbError::Busy) => ApiError::busy(&ctx),
            _ => ApiError::internal(&ctx),
        }
    })?.ok_or_else(|| invalid(&ctx))?;
    Ok(api::ok(&ctx, json!({"cards": created})))
}
