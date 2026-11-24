use std::collections::HashSet;

use axum::{Extension, extract::State, http::StatusCode, response::Response};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{db_error, invalid};
use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth::{self, ANALYST_SCOPE};
use crate::shared::clock::kst;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const INSERT: SqlStmt = sql_stmt!("admin.insert_analyst", "sql/insert_analyst.sql");
const SELECT: SqlStmt = sql_stmt!("admin.select_analyst", "sql/select_analyst.sql");
const INSERT_TOKEN: SqlStmt = sql_stmt!("admin.insert_token", "sql/insert_token.sql");
const TOKEN_TTL: Duration = Duration::minutes(30);
const MAX_BATCH: usize = 100;

#[derive(Deserialize)]
pub(super) struct Credentials {
    analyst_no: String,
    password: String,
}

impl Credentials {
    fn valid(&self) -> bool {
        (1..=40).contains(&self.analyst_no.len())
            && self
                .analyst_no
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            && (8..=128).contains(&self.password.chars().count())
    }
}

#[derive(Deserialize)]
struct Analyst {
    #[serde(flatten)]
    credentials: Credentials,
    name: String,
}

#[derive(Deserialize)]
pub(super) struct Batch {
    analysts: Vec<Analyst>,
}

pub(super) async fn seed(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    api::Json(batch): api::Json<Batch>,
) -> Result<Response, ApiError> {
    if !(1..=MAX_BATCH).contains(&batch.analysts.len()) {
        return Err(invalid(&ctx));
    }

    let mut seen = HashSet::new();
    for row in &batch.analysts {
        if !row.credentials.valid() || row.name.trim().is_empty() || row.name.chars().count() > 60 {
            return Err(invalid(&ctx));
        }
        if !seen.insert(row.credentials.analyst_no.as_str()) {
            return Err(duplicate(&ctx));
        }
    }

    let mut prepared = Vec::with_capacity(batch.analysts.len());
    for row in batch.analysts {
        let Credentials {
            analyst_no,
            password,
        } = row.credentials;
        let hash = auth::hash_secret(password)
            .await
            .map_err(|_| ApiError::internal(&ctx))?;
        prepared.push((analyst_no, row.name, hash));
    }

    let created = prepared.len();
    state
        .db
        .run_in(&stats, move |s| {
            for (number, name, hash) in prepared {
                s.execute(&INSERT, &[&number, &name, &hash])?;
            }
            s.commit()
        })
        .await
        .map_err(|error| {
            if error.ora_code() == Some(1) {
                duplicate(&ctx)
            } else {
                db_error(&ctx, &error)
            }
        })?;
    Ok(api::ok(&ctx, json!({"created": created})))
}

fn duplicate(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::CONFLICT,
        codes::DUPLICATE_REQUEST,
        "이미 존재합니다",
    )
}

fn unauthorized(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::UNAUTHORIZED,
        codes::UNAUTHORIZED,
        "인증이 필요합니다",
    )
}

pub(super) async fn login(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    api::Json(credentials): api::Json<Credentials>,
) -> Result<Response, ApiError> {
    if !credentials.valid() {
        return Err(invalid(&ctx));
    }

    let Credentials {
        analyst_no,
        password,
    } = credentials;
    let known: Option<(i64, String)> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&SELECT, &[&analyst_no]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let (analyst_id, hash) = match known {
        Some((id, hash)) => (Some(id), hash),
        None => (None, auth::dummy_hash().to_owned()),
    };
    let verified = auth::verify_secret(hash, password)
        .await
        .map_err(|_| ApiError::internal(&ctx))?;
    let issued = analyst_id.filter(|_| verified).map(|id| {
        let (token, hash) = auth::new_token();
        (id, token, hash)
    });
    let accepted = issued.is_some();

    // Analyst bearer lifetime is independent of the simulation.
    let expiry = Utc::now().with_timezone(&kst()) + TOKEN_TTL;
    let audit_ctx = RequestContext {
        device_id: None,
        ..ctx.clone()
    };
    let access_token = state
        .db
        .run_in(&stats, move |s| {
            let access_token = if let Some((id, token, hash)) = issued {
                s.execute(&INSERT_TOKEN, &[&hash, &id, &ANALYST_SCOPE, &expiry])?;
                Some(token)
            } else {
                None
            };
            let (kind, outcome, status, code) = if accepted {
                ("analyst.login.succeeded", "completed", 200, codes::SUCCESS)
            } else {
                ("analyst.login.failed", "rejected", 401, codes::UNAUTHORIZED)
            };
            let event = Event::for_request(&audit_ctx, kind, now, "ADMIN", outcome)
                .codes(code, None)
                .payload(json!({"analyst_id": analyst_id.map(|id| id.to_string())}));
            outbox::record(s, &event)?;
            outbox::complete(s, &audit_ctx, &event, status, code)?;
            s.commit()?;
            Ok::<_, DbError>(access_token)
        })
        .await
        .map_err(|error| db_error(&ctx, &error))?;
    let access_token = access_token.ok_or_else(|| unauthorized(&ctx))?;

    let mut response = api::ok(
        &ctx,
        json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "scope": ANALYST_SCOPE,
            "expires_in": TOKEN_TTL.num_seconds(),
        }),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
