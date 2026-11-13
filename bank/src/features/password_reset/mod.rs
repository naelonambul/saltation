//! Password reset and the generator-only simulated SMS reader.
use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Duration, FixedOffset};
use rand::Rng;
use serde::Deserialize;
use serde_json::json;
const SELECT_CUSTOMER: SqlStmt =
    sql_stmt!("password_reset.select_customer", "sql/select_customer.sql");
const PEEK_CODE: SqlStmt = sql_stmt!("password_reset.peek_code", "sql/peek_code.sql");
const INSERT_CODE: SqlStmt = sql_stmt!("password_reset.insert_code", "sql/insert_code.sql");
const INSERT_SMS: SqlStmt = sql_stmt!("password_reset.insert_sms", "sql/insert_sms.sql");
const SELECT_CODE: SqlStmt = sql_stmt!("password_reset.select_code", "sql/select_code.sql");
const FAIL_CODE: SqlStmt = sql_stmt!("password_reset.fail_code", "sql/fail_code.sql");
const USE_CODE: SqlStmt = sql_stmt!("password_reset.use_code", "sql/use_code.sql");
const CHANGE_PASSWORD: SqlStmt =
    sql_stmt!("password_reset.change_password", "sql/change_password.sql");
const END_SESSIONS: SqlStmt = sql_stmt!("password_reset.end_sessions", "sql/end_sessions.sql");
const REVOKE_TOKENS: SqlStmt = sql_stmt!("password_reset.revoke_tokens", "sql/revoke_tokens.sql");
const SELECT_SMS: SqlStmt = sql_stmt!("password_reset.select_sms", "sql/select_sms.sql");
const CHANNEL: &str = "MOBILE";
const RESET_REJECTED: &str = "O0002";
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/ext/auth/password_reset/request", post(request))
        .route("/v2.0/ext/auth/password_reset/confirm", post(confirm))
}
pub fn sms_router() -> Router<AppState> {
    Router::new().route("/internal/v1/sms/{phone}", get(sms))
}
fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::error!(request_id = %ctx.request_id, error = %e, "password reset database error");
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
fn valid_phone(phone: &str) -> bool {
    !phone.is_empty() && phone.chars().count() <= 20
}
#[derive(Deserialize)]
struct Request {
    phone: String,
}
async fn request(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    req: Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(req) = req.map_err(|_| invalid(&ctx))?;
    if !valid_phone(&req.phone) {
        return Err(invalid(&ctx));
    }
    // ThreadRng is the same cryptographic generator used by new_token.
    let code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
    // Reset codes are stored as Argon2id hashes (spec: Security); hashed for every phone alike.
    let hash = auth::hash_secret(code.clone())
        .await
        .map_err(|_| ApiError::internal(&ctx))?;
    let c = ctx.clone();
    state
        .db
        .run_in(&stats, move |s| {
            let customer: Option<(i64, String)> = s.query_opt(&SELECT_CUSTOMER, &[&req.phone])?;
            let mut event = Event::for_request(
                &c,
                "password_reset.requested",
                now,
                CHANNEL,
                if customer.is_some() {
                    "completed"
                } else {
                    "rejected"
                },
            )
            .codes(codes::SUCCESS, None);
            if let Some((id, no)) = customer {
                // Only the newest code of a customer is ever considered, so this one replaces the others.
                s.execute(
                    &INSERT_CODE,
                    &[&id, &hash, &now, &(now + Duration::minutes(5))],
                )?;
                s.execute(
                    &INSERT_SMS,
                    &[
                        &req.phone,
                        &format!("[살테이션] 비밀번호 재설정 인증번호 {code}"),
                        &now,
                    ],
                )?;
                event = event.customer(no).payload(json!({"reason": "issued"}));
            } else {
                event = event.payload(json!({"reason": "unknown"}));
            }
            outbox::record(s, &event)?;
            s.commit()
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, json!({})))
}
#[derive(Deserialize)]
struct Confirm {
    phone: String,
    code: String,
    new_password: String,
}
type CodeRow = (
    i64,
    Option<DateTime<FixedOffset>>,
    DateTime<FixedOffset>,
    i64,
);
async fn confirm(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    req: Result<Json<Confirm>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(req) = req.map_err(|_| invalid(&ctx))?;
    if !valid_phone(&req.phone)
        || req.code.len() != 6
        || !req.code.bytes().all(|b| b.is_ascii_digit())
        || !(8..=128).contains(&req.new_password.chars().count())
    {
        return Err(invalid(&ctx));
    }
    // Read the newest code without a lock, verify it outside the database, then decide under a lock.
    let phone = req.phone.clone();
    let seen: Option<(i64, String)> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&PEEK_CODE, &[&phone]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let (seen_reset, verified) = match seen {
        Some((reset, hash)) => (
            Some(reset),
            auth::verify_secret(hash, req.code.clone())
                .await
                .map_err(|_| ApiError::internal(&ctx))?,
        ),
        None => {
            // The same hashing work when there is nothing to verify against.
            auth::hash_secret(req.code.clone())
                .await
                .map_err(|_| ApiError::internal(&ctx))?;
            (None, false)
        }
    };
    let password_hash = auth::hash_secret(req.new_password)
        .await
        .map_err(|_| ApiError::internal(&ctx))?;
    let c = ctx.clone();
    let success = state
        .db
        .run_in(&stats, move |s| {
            let customer: Option<(i64, String)> = s.query_opt(&SELECT_CUSTOMER, &[&req.phone])?;
            let mut reason = "unknown";
            let mut success = false;
            if let Some((id, _)) = &customer {
                let row: Option<CodeRow> = s.query_opt(&SELECT_CODE, &[id])?;
                reason = "missing";
                if let Some((reset, used, expires, failures)) = row {
                    reason = if Some(reset) != seen_reset {
                        // A newer code was issued while this request was being verified.
                        "superseded"
                    } else if used.is_some() {
                        "used"
                    } else if expires <= now {
                        "expired"
                    } else if failures >= 5 {
                        "voided"
                    } else if !verified {
                        s.execute(&FAIL_CODE, &[&reset])?;
                        "incorrect"
                    } else {
                        s.execute(&CHANGE_PASSWORD, &[&password_hash, &now, id])?;
                        s.execute(&USE_CODE, &[&now, &reset])?;
                        // Tokens before sessions: the order the token check locks them in.
                        s.execute(&REVOKE_TOKENS, &[&now, id])?;
                        s.execute(&END_SESSIONS, &[&now, id])?;
                        success = true;
                        "completed"
                    };
                }
            }
            let mut event = Event::for_request(
                &c,
                if success {
                    "password_reset.completed"
                } else {
                    "password_reset.failed"
                },
                now,
                CHANNEL,
                if success { "completed" } else { "rejected" },
            )
            .codes(
                if success {
                    codes::SUCCESS
                } else {
                    RESET_REJECTED
                },
                None,
            )
            .payload(json!({"reason": reason}));
            if let Some((_, no)) = customer {
                event = event.customer(no);
            }
            outbox::record(s, &event)?;
            s.commit()?;
            Ok(success)
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    if success {
        Ok(api::ok(&ctx, json!({})))
    } else {
        Err(ApiError::new(
            &ctx,
            StatusCode::UNAUTHORIZED,
            RESET_REJECTED,
            "인증번호 불일치 또는 만료",
        ))
    }
}
async fn sms(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Path(phone): Path<String>,
) -> Result<Response, ApiError> {
    let key = phone.clone();
    let found: Option<(String, DateTime<FixedOffset>)> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&SELECT_SMS, &[&key]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let (message, occurred_at) = found
        .ok_or_else(|| ApiError::new(&ctx, StatusCode::NOT_FOUND, codes::NOT_FOUND, "대상 없음"))?;
    let code = message.rsplit(' ').next().unwrap_or_default();
    Ok(api::ok(
        &ctx,
        json!({"phone": phone, "message": message, "code": code, "occurred_at": occurred_at.to_rfc3339()}),
    ))
}
