//! Authentication primitives (spec F-2, F-3, N-6): Argon2id password hashing
//! behind its own small concurrency gate, opaque bearer tokens stored as
//! SHA-256 hashes, and the customer bearer-token extractor.

use std::sync::OnceLock;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Duration;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::app::AppState;
use crate::shared::api::{ApiError, codes};
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::SqlStmt;
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const SELECT_TOKEN: SqlStmt = sql_stmt!("auth.select_token", "sql/select_token.sql");
const DEFERRED_TOUCH: SqlStmt = sql_stmt!("auth.deferred_touch", "sql/deferred_touch.sql");

/// Session and token lifetime: 30 minutes of business-time inactivity.
pub const SESSION_IDLE: Duration = Duration::minutes(30);
/// Scopes granted to a customer login.
pub const CUSTOMER_SCOPE: &str = "login inquiry transfer";

fn hasher() -> Argon2<'static> {
    // OWASP minimum for Argon2id: 19 MiB, 2 iterations, 1 lane (spec N-6).
    let params = Params::new(19 * 1024, 2, 1, None).expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(2))
}

/// Hashes a secret (password or PIN) with Argon2id on a blocking thread.
pub async fn hash_secret(secret: String) -> anyhow::Result<String> {
    let _permit = gate().acquire().await?;
    tokio::task::spawn_blocking(move || {
        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("salt: {e}"))?;
        hasher()
            .hash_password(secret.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hash: {e}"))
    })
    .await?
}

/// Verifies a secret against a stored Argon2id hash on a blocking thread.
pub async fn verify_secret(hash: String, secret: String) -> anyhow::Result<bool> {
    let _permit = gate().acquire().await?;
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
        Ok(hasher().verify_password(secret.as_bytes(), &parsed).is_ok())
    })
    .await?
}

/// A new opaque bearer token and the hash stored for it.
pub fn new_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Open Banking `user_seq_no`, AN(10).
pub fn user_seq_no(customer_id: i64) -> String {
    format!("{customer_id:010}")
}

/// Request-local activity, applied only with its committed completion.
#[derive(Clone)]
pub struct PendingTouch {
    pub token_hash: String,
    pub session_id: String,
    pub now: chrono::DateTime<chrono::FixedOffset>,
}

impl std::fmt::Debug for PendingTouch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingTouch { .. }")
    }
}

impl PendingTouch {
    pub(crate) fn apply(
        &self,
        session: &mut crate::shared::db::Session,
    ) -> Result<(), crate::shared::db::DbError> {
        session
            .execute(
                &DEFERRED_TOUCH,
                &[&self.token_hash, &self.session_id, &self.now],
            )
            .map(|_| ())
            .map_err(|error| {
                // execute/timed records the original Oracle failure before remapping.
                if error.ora_code() == Some(30006) {
                    crate::shared::db::DbError::Busy
                } else {
                    error
                }
            })
    }
}

/// The authenticated customer of a request.
#[derive(Debug, Clone)]
pub struct AuthCustomer {
    pub device_id: Option<String>,
    pub customer_id: i64,
    pub customer_no: String,
    pub scope: String,
    pub session_id: String,
    pub token_hash: String,
}

impl AuthCustomer {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scope.split(' ').any(|s| s == scope)
    }
}

fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

impl FromRequestParts<AppState> for AuthCustomer {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = RequestContext::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});
        let unauthorized = || {
            ApiError::new(
                &ctx,
                StatusCode::UNAUTHORIZED,
                codes::UNAUTHORIZED,
                "인증이 필요합니다",
            )
        };
        let hash = token_hash(bearer(parts).ok_or_else(unauthorized)?);
        let stats = parts
            .extensions
            .get::<RequestStats>()
            .cloned()
            .unwrap_or_default();
        let BusinessTime(now) = BusinessTime::from_request_parts(parts, state).await?;
        let found = state
            .db
            .run_in(&stats, move |s| {
                let row: Option<(i64, String, String, String, Option<String>)> =
                    s.query_opt(&SELECT_TOKEN, &[&hash, &now])?;
                let Some((customer_id, customer_no, scope, session_id, device_id)) = row else {
                    return Ok(None);
                };
                Ok(Some(AuthCustomer {
                    device_id,
                    customer_id,
                    customer_no,
                    scope,
                    session_id,
                    token_hash: hash,
                }))
            })
            .await
            .map_err(|e| {
                if matches!(e, crate::shared::db::DbError::Busy) {
                    ApiError::busy(&ctx)
                } else {
                    tracing::error!(request_id = %ctx.request_id, error = %e, "customer token lookup failed");
                    ApiError::internal(&ctx)
                }
            })?;
        let customer = found.ok_or_else(unauthorized)?;
        stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending_touch = Some(PendingTouch {
            token_hash: customer.token_hash.clone(),
            session_id: customer.session_id.clone(),
            now,
        });
        if let Some(slot) = parts
            .extensions
            .get::<crate::shared::outcome::IdentitySlot>()
        {
            let mut identity = slot.lock().unwrap_or_else(|e| e.into_inner());
            identity.device_id = customer.device_id.clone();
            identity.customer_no = Some(customer.customer_no.clone());
            identity.session_id = Some(customer.session_id.clone());
        }
        Ok(customer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_verify_and_reject() {
        let hash = hash_secret("correct horse".into()).await.expect("hash");
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify_secret(hash.clone(), "correct horse".into())
                .await
                .expect("verify")
        );
        assert!(!verify_secret(hash, "wrong".into()).await.expect("verify"));
    }

    #[test]
    fn tokens_are_opaque_and_hashed() {
        let (token, hash) = new_token();
        assert_eq!(token.len(), 43);
        assert_eq!(hash.len(), 64);
        assert_eq!(token_hash(&token), hash);
        assert_eq!(user_seq_no(42), "0000000042");
    }
}

/// A live counterparty token; no customer session is touched or extended.
#[derive(Debug, Clone)]
pub struct AuthClient {
    pub counterparty_client_id: i64,
    pub client_id: String,
    pub institution_code: String,
    pub scope: String,
}
impl AuthClient {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scope.split_whitespace().any(|s| s == scope)
    }
}
const SELECT_CLIENT_TOKEN: SqlStmt =
    sql_stmt!("auth.select_client_token", "sql/select_client_token.sql");
impl FromRequestParts<AppState> for AuthClient {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = RequestContext::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});
        let unauthorized = || {
            ApiError::new(
                &ctx,
                StatusCode::UNAUTHORIZED,
                codes::UNAUTHORIZED,
                "인증이 필요합니다",
            )
        };
        let hash = token_hash(bearer(parts).ok_or_else(unauthorized)?);
        let stats = parts
            .extensions
            .get::<RequestStats>()
            .cloned()
            .unwrap_or_default();
        let BusinessTime(now) = BusinessTime::from_request_parts(parts, state).await?;
        let row: Option<(i64, String, String, String)> = state
            .db
            .run_in(&stats, move |s| {
                s.query_opt(&SELECT_CLIENT_TOKEN, &[&hash, &now])
            })
            .await
            .map_err(|e| {
                if matches!(e, crate::shared::db::DbError::Busy) {
                    ApiError::busy(&ctx)
                } else {
                    ApiError::internal(&ctx)
                }
            })?;
        let client = row
            .map(
                |(counterparty_client_id, client_id, institution_code, scope)| Self {
                    counterparty_client_id,
                    client_id,
                    institution_code,
                    scope,
                },
            )
            .ok_or_else(unauthorized)?;
        if let Some(slot) = parts
            .extensions
            .get::<crate::shared::outcome::IdentitySlot>()
        {
            slot.lock().unwrap_or_else(|e| e.into_inner()).client_id =
                Some(client.client_id.clone());
        }
        Ok(client)
    }
}

pub(crate) const ANALYST_SCOPE: &str = "analyst:read";
const SELECT_ANALYST_TOKEN: SqlStmt =
    sql_stmt!("auth.select_analyst_token", "sql/select_analyst_token.sql");

/// An independent analyst identity. Validating it performs no session writes.
pub struct AuthAnalyst;

impl FromRequestParts<AppState> for AuthAnalyst {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = RequestContext::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});
        let unauthorized = || {
            ApiError::new(
                &ctx,
                StatusCode::UNAUTHORIZED,
                codes::UNAUTHORIZED,
                "인증이 필요합니다",
            )
        };
        let hash = token_hash(bearer(parts).ok_or_else(unauthorized)?);
        let stats = parts
            .extensions
            .get::<RequestStats>()
            .cloned()
            .unwrap_or_default();
        let analyst: Option<i64> = state
            .db
            .run_in(&stats, move |s| {
                // Sample after acquiring the session so waiting for the pool cannot extend a token.
                let now = chrono::Utc::now().with_timezone(&crate::shared::clock::kst());
                s.query_opt(&SELECT_ANALYST_TOKEN, &[&hash, &ANALYST_SCOPE, &now])
            })
            .await
            .map_err(|error| if matches!(error, crate::shared::db::DbError::Busy) {
                ApiError::busy(&ctx)
            } else {
                tracing::warn!(request_id = %ctx.request_id, %error, "analyst credential lookup failed");
                ApiError::internal(&ctx)
            })?;
        analyst.map(|_| Self).ok_or_else(unauthorized)
    }
}
