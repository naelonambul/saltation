//! Generator-only terminal sessions and serialized PIN attempts.
use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth;
use crate::shared::clock::kst;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::shared::outcome::{self, IdentitySlot};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
mod operations;
use axum::{Extension, Router};
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Timelike};
use serde::Deserialize;
use serde_json::json;

const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGF0aW9uLWR1bW15IQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const PIN_HASH: SqlStmt = sql_stmt!("atm.pin_hash", "sql/pin_hash.sql");
const LOCK_TERMINAL: SqlStmt = sql_stmt!("atm.lock_terminal", "sql/lock_terminal.sql");
const TERMINAL: SqlStmt = sql_stmt!("atm.terminal", "sql/terminal.sql");
const LOCK_CREDENTIAL: SqlStmt = sql_stmt!("atm.lock_credential", "sql/lock_credential.sql");
const SELECT_LOCK_STATUS: SqlStmt =
    sql_stmt!("atm.select_lock_status", "sql/select_lock_status.sql");
const UPDATE_CREDENTIAL: SqlStmt = sql_stmt!("atm.update_credential", "sql/update_credential.sql");
const OPEN_SESSION: SqlStmt = sql_stmt!("atm.open_session", "sql/open_session.sql");
const LOCK_SESSION: SqlStmt = sql_stmt!("atm.lock_session", "sql/lock_session.sql");
const END_SESSION: SqlStmt = sql_stmt!("atm.end_session", "sql/end_session.sql");
const INSERT_SESSION: SqlStmt = sql_stmt!("atm.insert_session", "sql/insert_session.sql");
type At = DateTime<FixedOffset>;
type Terminal = (String, String, String, String);
type SessionRow = (String, String, At, Option<At>, String, String);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/atm/v1/session", post(start))
        .route("/atm/v1/session/end", post(end))
        .route("/atm/v1/cash/withdraw", post(operations::withdraw))
        .route("/atm/v1/cash/deposit", post(operations::deposit))
        .route("/atm/v1/balance", get(operations::balance))
}
fn db_error(ctx: &RequestContext, error: DbError) -> ApiError {
    if matches!(error, DbError::Busy) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}
fn unauthorized(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::UNAUTHORIZED,
        codes::UNAUTHORIZED,
        "인증이 필요합니다",
    )
}
fn event(
    ctx: &RequestContext,
    now: At,
    terminal: &Terminal,
    account: &str,
    kind: &'static str,
) -> Event {
    Event::for_request(ctx, kind, now, "ATM", "completed")
        .account(account)
        .payload(json!({
            "terminal_id": terminal.0, "region": terminal.1, "latitude": terminal.2,
            "longitude": terminal.3, "account_no": account, "request_id": ctx.request_id
        }))
}
fn completion(
    ctx: &RequestContext,
    now: At,
    account: &str,
    status: u16,
    code: &str,
    bank: Option<&str>,
) -> Event {
    Event::for_request(
        ctx,
        "request.completed",
        now,
        "ATM",
        if status < 400 {
            "completed"
        } else {
            "rejected"
        },
    )
    .account(account)
    .codes(code, bank)
    .payload(json!({"http_status": status, "request_type": "ATM_SESSION"}))
}
fn end_row(
    s: &mut Session,
    ctx: &RequestContext,
    now: At,
    terminal: &Terminal,
    row: &SessionRow,
    reason: &'static str,
    ended: At,
) -> Result<(), DbError> {
    if s.execute(&END_SESSION, &[&ended, &reason, &row.0])? != 1 {
        return Err(DbError::Busy);
    }
    record_end(s, ctx, now, terminal, row, reason, ended)
}
fn record_end(
    s: &mut Session,
    ctx: &RequestContext,
    now: At,
    terminal: &Terminal,
    row: &SessionRow,
    reason: &'static str,
    ended: At,
) -> Result<(), DbError> {
    let mut e = event(ctx, now, terminal, &row.1, "atm.session_ended").session(&row.0);
    e.payload["reason"] = reason.into();
    e.payload["session_id"] = row.0.clone().into();
    e.payload["start_request_id"] = row.4.clone().into();
    e.payload["ended_at"] = ended.to_rfc3339().into();
    outbox::record(s, &e)
}
#[derive(Deserialize)]
struct Start {
    terminal_id: String,
    account_no: String,
    pin: String,
}
struct Rejection {
    status: StatusCode,
    code: &'static str,
    bank: Option<&'static str>,
}
impl Rejection {
    fn bank(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: codes::BANK_REJECTED,
            bank: Some(code),
        }
    }
    fn response(&self, ctx: &RequestContext) -> Response {
        let body = match self.bank {
            Some(code) => json!({"bank_rsp_code": code, "bank_rsp_message": match code {
                "403" => "계좌 상태 오류", "409" => "PIN 입력 횟수 초과", "410" => "PIN 불일치", _ => "계좌 없음"
            }}),
            None => json!({}),
        };
        api::respond(
            ctx,
            self.status,
            self.code,
            if self.bank.is_some() {
                "참가기관 거부"
            } else {
                "대상 없음"
            },
            body,
        )
    }
}
async fn start(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    api::Json(req): api::Json<Start>,
) -> Result<Response, ApiError> {
    if req.terminal_id.is_empty()
        || req.terminal_id.len() > 20
        || !req.terminal_id.bytes().all(|b| (0x21..=0x7e).contains(&b))
        || req.account_no.is_empty()
        || req.account_no.len() > 16
        || !req.account_no.bytes().all(|b| b.is_ascii_digit())
        || req.pin.len() != 4
        || !req.pin.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ApiError::new(
            &ctx,
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMETER,
            "요청 항목 오류",
        ));
    }
    // Match the persisted TIMESTAMP(6) precision in the returned expiry.
    let now = now
        .with_nanosecond(now.timestamp_subsec_micros() * 1000)
        .unwrap();
    let account = req.account_no.clone();
    let hash: Option<String> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&PIN_HASH, &[&account]))
        .await
        .map_err(|e| db_error(&ctx, e))?;
    let correct = auth::verify_secret(hash.unwrap_or_else(|| DUMMY_HASH.into()), req.pin)
        .await
        .map_err(|_| ApiError::internal(&ctx))?;
    let (token, hash) = auth::new_token();
    let id = ulid::Ulid::new().to_string();
    let expires = now + Duration::minutes(3);
    let context = ctx.clone();
    let rejected = state
        .db
        .run_in(&stats, move |s| {
            let terminal: Option<Terminal> = s.query_opt(&LOCK_TERMINAL, &[&req.terminal_id])?;
            let credential: Option<(String, i64, Option<At>)> =
                s.query_opt(&LOCK_CREDENTIAL, &[&req.account_no])?;
            let mut locked_attempt = false;
            let rejected = if terminal.is_none() {
                Some(Rejection {
                    status: StatusCode::NOT_FOUND,
                    code: codes::NOT_FOUND,
                    bank: None,
                })
            } else if let Some((status, mut count, mut locked)) = credential {
                if locked.is_some_and(|at| at <= now) {
                    count = 0;
                    locked = None;
                    s.execute(&UPDATE_CREDENTIAL, &[&count, &locked, &req.account_no])?;
                }
                if status == "CLOSED" {
                    Some(Rejection::bank("403"))
                } else if locked.is_some_and(|at| at > now) {
                    Some(Rejection::bank("409"))
                } else if !correct {
                    count += 1;
                    if count == 3 {
                        locked_attempt = true;
                        locked = Some(
                            kst()
                                .from_local_datetime(
                                    &now.with_timezone(&kst())
                                        .date_naive()
                                        .succ_opt()
                                        .ok_or_else(|| {
                                            DbError::Config("business date overflow".into())
                                        })?
                                        .and_hms_opt(0, 0, 0)
                                        .unwrap(),
                                )
                                .single()
                                .unwrap(),
                        );
                    }
                    s.execute(&UPDATE_CREDENTIAL, &[&count, &locked, &req.account_no])?;
                    Some(Rejection::bank("410"))
                } else {
                    s.execute(
                        &UPDATE_CREDENTIAL,
                        &[&0i64, &Option::<At>::None, &req.account_no],
                    )?;
                    None
                }
            } else {
                Some(Rejection::bank("404"))
            };
            if let Some(rejection) = rejected {
                let complete = completion(
                    &context,
                    now,
                    &req.account_no,
                    rejection.status.as_u16(),
                    rejection.code,
                    rejection.bank,
                );
                let (attempt, reused) = outcome::reject_attempt(
                    s,
                    &context,
                    &complete,
                    None,
                    terminal.as_ref().map(|t| t.0.as_str()),
                )?;
                if let Some(terminal) = &terminal {
                    let mut e = event(
                        &context,
                        now,
                        terminal,
                        &req.account_no,
                        "atm.session_rejected",
                    )
                    .codes(rejection.code, rejection.bank);
                    e.outcome = "rejected";
                    e.payload["bank_rsp_code"] = json!(rejection.bank);
                    e.payload["locked"] = locked_attempt.into();
                    outbox::record(s, &e)?;
                }
                let mut complete = completion(
                    &context,
                    now,
                    &req.account_no,
                    rejection.status.as_u16(),
                    rejection.code,
                    rejection.bank,
                );
                if reused {
                    complete.payload["attempt_id"] = attempt.into();
                }
                outbox::record(s, &complete)?;
                s.commit()?;
                return Ok(Some(rejection));
            }
            let terminal = terminal.ok_or(DbError::Busy)?;
            let open: Option<SessionRow> = s.query_opt(&OPEN_SESSION, &[&terminal.0])?;
            if let Some(row) = open {
                let (reason, at) = if row.2 <= now {
                    ("expired", row.2)
                } else {
                    ("replaced", now)
                };
                if s.execute(&END_SESSION, &[&at, &reason, &row.0])? == 1 {
                    record_end(s, &context, now, &terminal, &row, reason, at)?;
                }
            }
            s.execute(
                &INSERT_SESSION,
                &[
                    &id,
                    &terminal.0,
                    &req.account_no,
                    &hash,
                    &now,
                    &expires,
                    &context.request_id,
                ],
            )?;
            let mut e = event(
                &context,
                now,
                &terminal,
                &req.account_no,
                "atm.session_started",
            )
            .session(&id);
            e.payload["session_id"] = id.clone().into();
            outbox::record(s, &e)?;
            outbox::record(
                s,
                &completion(&context, now, &req.account_no, 200, codes::SUCCESS, None).session(&id),
            )?;
            s.commit()?;
            Ok(None)
        })
        .await
        .map_err(|e| db_error(&ctx, e))?;
    Ok(match rejected {
        Some(rejection) => rejection.response(&ctx),
        None => api::ok(
            &ctx,
            json!({"atm_session_token": token, "expires_at": expires.to_rfc3339()}),
        ),
    })
}
async fn end(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Extension(identity): Extension<IdentitySlot>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| unauthorized(&ctx))?;
    let hash = auth::token_hash(token);
    let context = ctx.clone();
    let success = state
        .db
        .run_in(&stats, move |s| {
            let found: Option<SessionRow> = s.query_opt(&LOCK_SESSION, &[&hash])?;
            let Some(row) = found else {
                return Ok(false);
            };
            {
                let mut identity = identity.lock().unwrap_or_else(|e| e.into_inner());
                identity.terminal_id = Some(row.5.clone());
                identity.session_id = Some(row.0.clone());
                identity.account_no = Some(row.1.clone());
            }
            if row.3.is_some() {
                return Ok(false);
            }
            let terminal: Terminal = s.query_row(&TERMINAL, &[&row.5])?;
            let expired = row.2 <= now;
            end_row(
                s,
                &context,
                now,
                &terminal,
                &row,
                if expired { "expired" } else { "ended" },
                if expired { row.2 } else { now },
            )?;
            outbox::record(
                s,
                &completion(
                    &context,
                    now,
                    &row.1,
                    if expired { 401 } else { 200 },
                    if expired {
                        codes::UNAUTHORIZED
                    } else {
                        codes::SUCCESS
                    },
                    None,
                )
                .session(&row.0),
            )?;
            s.commit()?;
            Ok(!expired)
        })
        .await
        .map_err(|e| db_error(&ctx, e))?;
    if success {
        Ok(api::ok(&ctx, json!({})))
    } else {
        Err(unauthorized(&ctx))
    }
}

/// End sessions that have reached their expiry by the business date cutoff.
pub struct AtmSessionExpiryJob;

impl crate::shared::rollover::RolloverJob for AtmSessionExpiryJob {
    fn name(&self) -> &'static str {
        "atm_session_expiry"
    }

    fn run<'a>(
        &'a self,
        db: &'a crate::shared::db::Db,
        _date: chrono::NaiveDate,
        at: At,
    ) -> crate::shared::rollover::JobFuture<'a> {
        Box::pin(async move {
            const CANDIDATES: SqlStmt =
                sql_stmt!("atm.expiry_candidates", "sql/expiry_candidates.sql");
            const EXPIRE: SqlStmt = sql_stmt!("atm.expire_session", "sql/expire_session.sql");
            let rows: Vec<(String, String, At, String, String)> =
                db.run(move |s| s.query_all(&CANDIDATES, &[&at])).await?;
            for (id, account, expires, start_request, terminal_id) in rows {
                db.run(move |s| {
                    if s.execute(&EXPIRE, &[&id])? == 1 {
                        let terminal: Terminal = s.query_row(&TERMINAL, &[&terminal_id])?;
                        let e = Event::system("atm.session_ended", at, "completed")
                            .account(&account)
                            .session(&id)
                            .payload(json!({
                                "terminal_id": terminal.0, "region": terminal.1,
                                "latitude": terminal.2, "longitude": terminal.3,
                                "account_no": account, "request_id": null,
                                "reason": "expired", "session_id": id,
                                "start_request_id": start_request,
                                "ended_at": expires.to_rfc3339()
                            }));
                        outbox::record(s, &e)?;
                    }
                    s.commit()
                })
                .await?;
            }
            Ok(())
        })
    }
}
