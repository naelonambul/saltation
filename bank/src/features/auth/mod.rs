//! Customer login, logout and lockout (spec F-2 to F-4).
//! Password verification runs outside any database session (Argon2 is slow);
//! the outcome is then recorded in one transaction with its outbox events.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Duration, FixedOffset};
use serde::Deserialize;
use serde_json::json;
use ulid::Ulid;

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth::{self, AuthCustomer, CUSTOMER_SCOPE, SESSION_IDLE};
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const SELECT_LOGIN: SqlStmt = sql_stmt!("auth.select_login", "sql/select_login.sql");
const LOCK_CREDENTIAL: SqlStmt = sql_stmt!("auth.lock_credential", "sql/lock_credential.sql");
const RECORD_FAILURE: SqlStmt = sql_stmt!("auth.record_failure", "sql/record_failure.sql");
const SELECT_FAILURES: SqlStmt = sql_stmt!("auth.select_failures", "sql/select_failures.sql");
const RESET_FAILURES: SqlStmt = sql_stmt!("auth.reset_failures", "sql/reset_failures.sql");
const SELECT_DEVICE: SqlStmt = sql_stmt!("auth.select_device", "sql/select_device.sql");
const INSERT_DEVICE: SqlStmt = sql_stmt!("auth.insert_device", "sql/insert_device.sql");
const TOUCH_DEVICE: SqlStmt = sql_stmt!("auth.touch_device", "sql/touch_device.sql");
const INSERT_SESSION: SqlStmt = sql_stmt!("auth.insert_session", "sql/insert_session.sql");
const INSERT_TOKEN: SqlStmt = sql_stmt!("auth.insert_token", "sql/insert_token.sql");
const INSERT_ATTEMPT: SqlStmt =
    sql_stmt!("auth.insert_login_attempt", "sql/insert_login_attempt.sql");
const REVOKE_TOKEN: SqlStmt = sql_stmt!("auth.revoke_token", "sql/revoke_token.sql");
const END_SESSION: SqlStmt = sql_stmt!("auth.end_session", "sql/end_session.sql");

const LOCK_FOR: Duration = Duration::minutes(30);
const CHANNEL: &str = "MOBILE";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/ext/auth/login", post(login))
        .route("/oauth/2.0/revoke", post(logout))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    phone: String,
    password: String,
    device_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Success,
    UnknownPhone,
    WrongPassword,
    Locked,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "SUCCESS",
            Outcome::UnknownPhone => "UNKNOWN_PHONE",
            Outcome::WrongPassword => "WRONG_PASSWORD",
            Outcome::Locked => "LOCKED",
        }
    }
}

type LoginRow = (i64, String, String, i64, Option<DateTime<FixedOffset>>);

fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::error!(request_id = %ctx.request_id, error = %e, "auth database error");
    if matches!(e, DbError::Busy) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}

async fn login(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let device_id = req.device_id.clone().or_else(|| ctx.device_id.clone());
    let phone = req.phone.clone();
    let found: Option<LoginRow> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&SELECT_LOGIN, &[&phone]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;

    let outcome = match &found {
        None => Outcome::UnknownPhone,
        Some((.., Some(until))) if *until > now => Outcome::Locked,
        Some((_, _, hash, ..)) => {
            let ok = auth::verify_secret(hash.clone(), req.password.clone()).await.map_err(|e| {
                tracing::error!(request_id = %ctx.request_id, error = %e, "password verification failed");
                ApiError::internal(&ctx)
            })?;
            if ok {
                Outcome::Success
            } else {
                Outcome::WrongPassword
            }
        }
    };

    let verified_hash = found.as_ref().map(|(_, _, hash, ..)| hash.clone());
    let customer = found.map(|(id, no, _, _, locked)| (id, no, locked));
    let (token, token_hash) = auth::new_token();
    let session_id = Ulid::new().to_string();
    let c = ctx.clone();
    let phone = req.phone.clone();
    let dev = device_id.clone();
    let sid = session_id.clone();
    let (failures, customer_id): (i64, Option<i64>) = state
        .db
        .run_in(&stats, move |s| {
            let customer_id = customer.as_ref().map(|(id, ..)| *id);
            let mut failures = 0;
            let mut event = Event::for_request(&c, "login.failed", now, CHANNEL, "rejected")
                .device(dev.clone())
                .codes(codes::UNAUTHORIZED, None);
            if let Some((id, no, locked)) = &customer {
                event = event.customer(no.clone());
                // An expired lock starts a fresh count.
                if locked.is_some_and(|until| until <= now) {
                    s.execute(&RESET_FAILURES, &[id])?;
                }
                match outcome {
                    Outcome::WrongPassword => {
                        s.execute(&RECORD_FAILURE, &[&(now + LOCK_FOR), id])?;
                        let (count, _): (i64, Option<DateTime<FixedOffset>>) =
                            s.query_row(&SELECT_FAILURES, &[id])?;
                        failures = count;
                    }
                    Outcome::Success => {
                        // The password was verified before this transaction; if it changed
                        // since (a reset), the caller retries and is judged on the new one.
                        let current: String = s.query_row(&LOCK_CREDENTIAL, &[id])?;
                        if Some(&current) != verified_hash.as_ref() {
                            s.rollback()?;
                            return Err(DbError::Busy);
                        }
                        s.execute(&RESET_FAILURES, &[id])?;
                        let mut new_device = false;
                        if let Some(d) = &dev {
                            let seen: i64 = s.query_row(&SELECT_DEVICE, &[id, d])?;
                            if seen == 0 {
                                s.execute(&INSERT_DEVICE, &[id, d, &now, &now])?;
                                new_device = true;
                            } else {
                                s.execute(&TOUCH_DEVICE, &[&now, id, d])?;
                            }
                        }
                        let human = if c.human { "Y" } else { "N" };
                        s.execute(
                            &INSERT_SESSION,
                            &[
                                &sid,
                                id,
                                &dev,
                                &c.listener.as_str(),
                                &human,
                                &c.ip,
                                &c.region,
                                &now,
                                &now,
                            ],
                        )?;
                        s.execute(
                            &INSERT_TOKEN,
                            &[
                                &token_hash,
                                id,
                                &CUSTOMER_SCOPE,
                                &sid,
                                &(now + SESSION_IDLE),
                            ],
                        )?;
                        event =
                            Event::for_request(&c, "login.succeeded", now, CHANNEL, "completed")
                                .customer(no.clone())
                                .session(sid.clone())
                                .device(dev.clone())
                                .codes(codes::SUCCESS, None);
                        if new_device {
                            let device_event =
                                Event::for_request(&c, "device.new", now, CHANNEL, "completed")
                                    .customer(no.clone())
                                    .session(sid.clone())
                                    .device(dev.clone());
                            outbox::record(s, &device_event)?;
                        }
                    }
                    Outcome::Locked | Outcome::UnknownPhone => {}
                }
            }
            event = event.payload(json!({ "outcome": outcome.as_str(), "failed_count": failures }));
            s.execute(
                &INSERT_ATTEMPT,
                &[
                    &phone,
                    &customer_id,
                    &outcome.as_str(),
                    &dev,
                    &c.ip,
                    &c.region,
                    &c.request_id,
                    &now,
                ],
            )?;
            outbox::record(s, &event)?;
            s.commit()?;
            Ok((failures, customer_id))
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;

    match outcome {
        Outcome::Success => Ok(api::ok(
            &ctx,
            json!({
                "access_token": token,
                "token_type": "Bearer",
                "expires_in": SESSION_IDLE.num_seconds(),
                "scope": CUSTOMER_SCOPE,
                "user_seq_no": auth::user_seq_no(customer_id.unwrap_or_default()),
            }),
        )),
        Outcome::Locked => Err(ApiError::new(
            &ctx,
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHORIZED,
            "로그인 5회 실패로 계정이 잠겼습니다. 30분 후 다시 시도해 주세요",
        )),
        Outcome::WrongPassword if failures >= 5 => Err(ApiError::new(
            &ctx,
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHORIZED,
            "로그인 5회 실패로 계정이 잠겼습니다. 30분 후 다시 시도해 주세요",
        )),
        Outcome::UnknownPhone | Outcome::WrongPassword => Err(ApiError::new(
            &ctx,
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHORIZED,
            "전화번호 또는 비밀번호가 올바르지 않습니다",
        )),
    }
}

async fn logout(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
) -> Result<Response, ApiError> {
    let c = ctx.clone();
    state
        .db
        .run_in(&stats, move |s| {
            s.execute(&REVOKE_TOKEN, &[&now, &who.token_hash])?;
            s.execute(&END_SESSION, &[&now, &who.session_id])?;
            let event = Event::for_request(&c, "logout", now, CHANNEL, "completed")
                .customer(who.customer_no.clone())
                .session(who.session_id.clone());
            outbox::record(s, &event)?;
            s.commit()
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(&ctx, json!({})))
}
