//! Request completion metadata and the final, fail-closed outbox write.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{FromRequestParts, MatchedPath, Request};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset};

use crate::app::AppState;
use crate::shared::api::ApiError;
use crate::shared::context::RequestContext;
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;

#[derive(Clone, Debug, Default)]
pub struct ResponseMetadata {
    pub rsp_code: Option<String>,
    pub bank_rsp_code: Option<String>,
    pub account_no: Option<String>,
    pub request_type: Option<String>,
    pub replayed: Option<bool>,
}

/// Adds optional outcome fields while preserving the response's codes.
pub fn annotate(
    response: &mut Response,
    account_no: Option<String>,
    request_type: Option<String>,
    replayed: Option<bool>,
) {
    let metadata = response
        .extensions_mut()
        .get_or_insert_default::<ResponseMetadata>();
    metadata.account_no = account_no;
    metadata.request_type = request_type;
    metadata.replayed = replayed;
}

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub customer_no: Option<String>,
    pub session_id: Option<String>,
    pub client_id: Option<String>,
}
pub type IdentitySlot = Arc<Mutex<Identity>>;

#[derive(Clone, Copy)]
pub struct Arrival(pub Instant);
impl<S: Send + Sync> FromRequestParts<S> for Arrival {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx = RequestContext::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});
        parts
            .extensions
            .get::<Self>()
            .copied()
            .ok_or_else(|| ApiError::internal(&ctx))
    }
}

#[derive(Clone)]
pub struct Recorder {
    pub ctx: RequestContext,
    stats: RequestStats,
    identity: IdentitySlot,
    arrival: Arrival,
    endpoint: String,
    method: String,
    channel: &'static str,
    excluded: bool,
}
impl Recorder {
    pub fn attach(req: &mut Request, ctx: RequestContext) -> Self {
        let identity = IdentitySlot::default();
        req.extensions_mut().insert(identity.clone());
        let path = req.uri().path();
        let endpoint = req
            .extensions()
            .get::<MatchedPath>()
            .map(|p| p.as_str().to_owned())
            .unwrap_or_else(|| "unmatched".into());
        if let Some(stats) = req.extensions().get::<RequestStats>() {
            let mut stats = stats.lock().unwrap_or_else(|e| e.into_inner());
            stats.outcome_endpoint = Some(endpoint.clone());
            stats.outcome_method = Some(req.method().to_string());
            stats.outcome_arrival = req.extensions().get::<Arrival>().map(|a| a.0);
        }
        Self {
            ctx,
            stats: req
                .extensions()
                .get::<RequestStats>()
                .cloned()
                .unwrap_or_default(),
            identity,
            arrival: req
                .extensions()
                .get::<Arrival>()
                .copied()
                .unwrap_or(Arrival(Instant::now())),
            endpoint,
            method: req.method().to_string(),
            channel: if matches!(path, "/oauth/2.0/token" | "/v2.0/transfer/deposit/fin_num") {
                "EXTERNAL"
            } else {
                "MOBILE"
            },
            excluded: matches!(path, "/healthz" | "/metrics") || path.starts_with("/internal/"),
        }
    }

    pub fn unrecorded(&self) {
        if !self.excluded {
            metrics::counter!("bank_request_outcomes_unrecorded_total", "listener" => self.ctx.listener.as_str()).increment(1);
            tracing::warn!(request_id = %self.ctx.request_id, "request outcome was not recorded");
        }
    }

    pub async fn finish(
        &self,
        state: &AppState,
        now: DateTime<FixedOffset>,
        mut response: Response,
    ) -> Response {
        if self.excluded
            || self
                .stats
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .completion_committed
        {
            return response;
        }
        let metadata = response
            .extensions()
            .get::<ResponseMetadata>()
            .cloned()
            .unwrap_or_default();
        let identity = self
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut event = Event::for_request(
            &self.ctx,
            "request.completed",
            now,
            self.channel,
            if response.status().as_u16() < 400 {
                "completed"
            } else {
                "rejected"
            },
        );
        event.rsp_code = metadata.rsp_code;
        event.bank_rsp_code = metadata.bank_rsp_code;
        event.account_no = metadata.account_no;
        event.customer_no = identity.customer_no;
        event.session_id = identity.session_id;
        if event.device_id.is_none() {
            event.device_id = self.ctx.device_id.clone();
        }
        event.payload = serde_json::json!({
            "endpoint": self.endpoint,
            "method": self.method,
            "http_status": response.status().as_u16(),
            "duration_ms": self.arrival.0.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        });
        if let Some(kind) = metadata.request_type {
            event.payload["request_type"] = kind.into();
        }
        if let Some(replayed) = metadata.replayed {
            event.payload["replayed"] = replayed.into();
        }
        let operation = if matches!(response.status().as_u16(), 401 | 403) {
            money_operation(&self.method, &self.endpoint)
        } else {
            None
        };
        let ctx = self.ctx.clone();
        if state
            .db
            .run_in(&self.stats, move |s| {
                if let Some((kind, channel)) = operation {
                    event.channel = channel;
                    event.payload["request_type"] = kind.into();
                    let (id, reused) = reject_attempt(s, &ctx, &event, None)?;
                    if reused {
                        event.payload["attempt_id"] = id.into();
                    }
                }
                outbox::record(s, &event)?;
                s.commit()
            })
            .await
            .is_err()
        {
            self.unrecorded();
            if response.status().as_u16() < 500 {
                response = ApiError::busy(&self.ctx).into_response();
            }
        }
        response
    }
}

/// Records an unclaimed rejection in the caller's transaction. The event supplies
/// the response codes, request type, identity and business time.
pub fn reject_attempt(
    session: &mut crate::shared::db::Session,
    ctx: &RequestContext,
    event: &Event,
    bank_tran_id: Option<&str>,
) -> Result<(String, bool), crate::shared::db::DbError> {
    use crate::shared::db::{DbError, SqlStmt};
    const INSERT: SqlStmt = crate::sql_stmt!("outcome.insert_rejected", "sql/insert_rejected.sql");
    let at = DateTime::parse_from_rfc3339(&event.occurred_at)
        .map_err(|e| DbError::Config(format!("attempt time: {e}")))?;
    let kind = event.payload["request_type"]
        .as_str()
        .ok_or_else(|| DbError::Config("rejected attempt needs request_type".into()))?;
    let insert = |s: &mut crate::shared::db::Session, id: &str, reused: Option<&str>| {
        s.execute(
            &INSERT,
            &[
                &id,
                &reused,
                &kind,
                &event.channel,
                &ctx.listener.as_str(),
                &event.rsp_code,
                &event.bank_rsp_code,
                &bank_tran_id,
                &event.session_id,
                &event.device_id,
                &ctx.ip,
                &ctx.region,
                &at,
            ],
        )
    };
    match insert(session, &ctx.request_id, None) {
        Ok(_) => Ok((ctx.request_id.clone(), false)),
        Err(e) if e.ora_code() == Some(1) => {
            let id = ulid::Ulid::new().to_string();
            insert(session, &id, Some(&ctx.request_id))?;
            Ok((id, true))
        }
        Err(e) => Err(e),
    }
}

/// Persists a conflict and its completion atomically, without claiming the key.
pub fn conflict(
    session: &mut crate::shared::db::Session,
    ctx: &RequestContext,
    now: DateTime<FixedOffset>,
    kind: &'static str,
    channel: &'static str,
    key: &str,
    identity: (Option<&str>, Option<&str>),
) -> Result<(), crate::shared::db::DbError> {
    let mut event = Event::for_request(ctx, "request.completed", now, channel, "rejected")
        .codes("A0007", None)
        .payload(serde_json::json!({"http_status": 409, "request_type": kind}));
    event.customer_no = identity.0.map(str::to_owned);
    event.session_id = identity.1.map(str::to_owned);
    let (id, reused) = reject_attempt(session, ctx, &event, Some(key))?;
    if reused {
        event.payload["attempt_id"] = id.into();
    }
    outbox::record(session, &event)?;
    session.commit()
}

fn money_operation(method: &str, route: &str) -> Option<(&'static str, &'static str)> {
    if method != "POST" {
        return None;
    }
    match route {
        "/v2.0/transfer/withdraw/fin_num" => Some(("TRANSFER", "MOBILE")),
        "/v2.0/transfer/deposit/fin_num" => Some(("INBOUND", "EXTERNAL")),
        "/v2.0/ext/deposits" => Some(("TERM_OPEN", "MOBILE")),
        "/v2.0/ext/deposits/{id}/close" => Some(("TERM_CLOSE", "MOBILE")),
        "/v2.0/ext/loans" => Some(("LOAN_TAKE", "MOBILE")),
        "/v2.0/ext/loans/{id}/repay" => Some(("LOAN_REPAY", "MOBILE")),
        _ => None,
    }
}
