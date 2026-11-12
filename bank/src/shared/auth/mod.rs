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
const EXTEND_TOKEN: SqlStmt = sql_stmt!("auth.extend_token", "sql/extend_token.sql");
const TOUCH_SESSION: SqlStmt = sql_stmt!("auth.touch_session", "sql/touch_session.sql");

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

/// The authenticated customer of a request.
#[derive(Debug, Clone)]
pub struct AuthCustomer {
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
                let row: Option<(i64, String, String, String)> =
                    s.query_opt(&SELECT_TOKEN, &[&hash, &now])?;
                let Some((customer_id, customer_no, scope, session_id)) = row else {
                    return Ok(None);
                };
                s.execute(&EXTEND_TOKEN, &[&(now + SESSION_IDLE), &hash])?;
                s.execute(&TOUCH_SESSION, &[&now, &session_id])?;
                s.commit()?;
                Ok(Some(AuthCustomer {
                    customer_id,
                    customer_no,
                    scope,
                    session_id,
                    token_hash: hash,
                }))
            })
            .await
            .map_err(|_| ApiError::internal(&ctx))?;
        found.ok_or_else(unauthorized)
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
