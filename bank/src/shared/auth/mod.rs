//! Authentication primitives (spec F-2, F-3, N-6): Argon2id password hashing
//! behind its own small concurrency gate, opaque bearer tokens stored as
//! SHA-256 hashes, and the customer bearer-token extractor.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

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

/// Configured Argon2id parameters (spec N-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Config {
    pub m_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub concurrency: usize,
}

impl Default for Argon2Config {
    fn default() -> Self {
        Self {
            m_kib: 19 * 1024,
            t_cost: 2,
            p_cost: 1,
            concurrency: 2,
        }
    }
}

impl Argon2Config {
    pub fn new(m_kib: u32, t_cost: u32, p_cost: u32, concurrency: usize) -> anyhow::Result<Self> {
        if !(4096..=65_536).contains(&m_kib) {
            anyhow::bail!("SALTATION_ARGON2_M_KIB must be between 4096 and 65536 (got {m_kib})");
        }
        if !(1..=10).contains(&t_cost) {
            anyhow::bail!("SALTATION_ARGON2_T_COST must be between 1 and 10 (got {t_cost})");
        }
        if !(1..=4).contains(&p_cost) {
            anyhow::bail!("SALTATION_ARGON2_P_COST must be between 1 and 4 (got {p_cost})");
        }
        if !(1..=8).contains(&concurrency) {
            anyhow::bail!(
                "SALTATION_ARGON2_CONCURRENCY must be between 1 and 8 (got {concurrency})"
            );
        }
        Params::new(m_kib, t_cost, p_cost, None)
            .map_err(|e| anyhow::anyhow!("invalid Argon2 parameters: {e}"))?;
        Ok(Self {
            m_kib,
            t_cost,
            p_cost,
            concurrency,
        })
    }

    pub fn from_env() -> anyhow::Result<Self> {
        fn read_u32(name: &str, default: u32) -> anyhow::Result<u32> {
            match std::env::var(name) {
                Ok(v) => v
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| anyhow::anyhow!("{name} must be an unsigned integer")),
                Err(std::env::VarError::NotPresent) => Ok(default),
                Err(e) => Err(anyhow::anyhow!("{name}: {e}")),
            }
        }
        let d = Self::default();
        let m_kib = read_u32("SALTATION_ARGON2_M_KIB", d.m_kib)?;
        let t_cost = read_u32("SALTATION_ARGON2_T_COST", d.t_cost)?;
        let p_cost = read_u32("SALTATION_ARGON2_P_COST", d.p_cost)?;
        let concurrency = read_u32("SALTATION_ARGON2_CONCURRENCY", d.concurrency as u32)? as usize;
        Self::new(m_kib, t_cost, p_cost, concurrency)
    }

    fn hasher(self) -> Argon2<'static> {
        let params =
            Params::new(self.m_kib, self.t_cost, self.p_cost, None).expect("validated Argon2");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }

    fn dummy_phc(self) -> String {
        let phc = format!(
            "$argon2id$v=19$m={},t={},p={}$c2FsdGF0aW9uLWR1bW15IQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            self.m_kib, self.t_cost, self.p_cost
        );
        PasswordHash::new(&phc).expect("valid dummy PHC hash");
        phc
    }
}

static ARGON2_CONFIG: OnceLock<Argon2Config> = OnceLock::new();
static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// Validates and locks Argon2id parameters from the environment at process startup.
pub fn init_argon2_from_env() -> anyhow::Result<Argon2Config> {
    let cfg = Argon2Config::from_env()?;
    let active = *ARGON2_CONFIG.get_or_init(|| cfg);
    let _ = GATE.get_or_init(|| Arc::new(Semaphore::new(active.concurrency)));
    let _ = DUMMY_HASH.get_or_init(|| active.dummy_phc());
    Ok(active)
}

fn config() -> Argon2Config {
    *ARGON2_CONFIG.get_or_init(|| Argon2Config::from_env().unwrap_or_default())
}

fn hasher() -> Argon2<'static> {
    config().hasher()
}

fn gate() -> &'static Arc<Semaphore> {
    GATE.get_or_init(|| Arc::new(Semaphore::new(config().concurrency)))
}

/// Valid Argon2id PHC hash matching the active N-6 parameters for constant-work rejection paths.
pub fn dummy_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| config().dummy_phc()).as_str()
}

async fn run_gated_blocking<T, F>(
    gate: &Arc<Semaphore>,
    op: &'static str,
    work: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    let wait_started = Instant::now();
    let permit = Arc::clone(gate).acquire_owned().await?;
    metrics::histogram!("bank_auth_gate_wait_seconds").record(wait_started.elapsed().as_secs_f64());
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let hash_started = Instant::now();
        let result = work();
        metrics::histogram!("bank_auth_hash_duration_seconds", "op" => op)
            .record(hash_started.elapsed().as_secs_f64());
        result
    })
    .await?
}

/// Hashes a secret (password or PIN) with Argon2id on a blocking thread.
pub async fn hash_secret(secret: String) -> anyhow::Result<String> {
    run_gated_blocking(gate(), "hash", move || {
        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("salt: {e}"))?;
        hasher()
            .hash_password(secret.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hash: {e}"))
    })
    .await
}

/// Verifies a secret against a stored Argon2id hash on a blocking thread.
pub async fn verify_secret(hash: String, secret: String) -> anyhow::Result<bool> {
    run_gated_blocking(gate(), "verify", move || {
        let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
        Ok(hasher().verify_password(secret.as_bytes(), &parsed).is_ok())
    })
    .await
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

    #[tokio::test]
    async fn cancelled_caller_keeps_gate_permit_until_blocking_work_finishes() {
        let gate = Arc::new(Semaphore::new(2));
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release1_tx, release1_rx) = std::sync::mpsc::channel::<()>();
        let (release2_tx, release2_rx) = std::sync::mpsc::channel::<()>();

        let g1 = Arc::clone(&gate);
        let s1 = started_tx.clone();
        let h1 = tokio::spawn(async move {
            run_gated_blocking(&g1, "verify", move || {
                let _ = s1.send(1);
                let _ = release1_rx.recv();
                Ok(())
            })
            .await
        });

        let g2 = Arc::clone(&gate);
        let s2 = started_tx.clone();
        let h2 = tokio::spawn(async move {
            run_gated_blocking(&g2, "verify", move || {
                let _ = s2.send(2);
                let _ = release2_rx.recv();
                Ok(())
            })
            .await
        });

        let _ = started_rx.recv().await;
        let _ = started_rx.recv().await;
        assert_eq!(gate.available_permits(), 0);

        // Cancelling the caller future must not free the permit while blocking work still runs.
        h1.abort();
        let _ = h1.await;
        assert_eq!(gate.available_permits(), 0);

        let g3 = Arc::clone(&gate);
        let s3 = started_tx;
        let h3 = tokio::spawn(async move {
            run_gated_blocking(&g3, "verify", move || {
                let _ = s3.send(3);
                Ok(())
            })
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), started_rx.recv())
                .await
                .is_err(),
            "third task must wait until a running blocking job finishes"
        );

        let _ = release1_tx.send(());
        assert_eq!(started_rx.recv().await, Some(3));
        let _ = release2_tx.send(());
        h2.await.expect("join h2").expect("h2 ok");
        h3.await.expect("join h3").expect("h3 ok");
        assert_eq!(gate.available_permits(), 2);
    }

    #[test]
    fn argon2_config_validates_bounds_and_encodes_phc() {
        assert!(Argon2Config::new(2048, 2, 1, 2).is_err());
        assert!(Argon2Config::new(4096, 0, 1, 2).is_err());
        assert!(Argon2Config::new(4096, 2, 0, 2).is_err());
        assert!(Argon2Config::new(4096, 2, 1, 0).is_err());
        assert!(Argon2Config::new(4096, 2, 1, 9).is_err());

        let cfg = Argon2Config::new(4096, 2, 1, 3).expect("valid config");
        let dummy = cfg.dummy_phc();
        assert!(dummy.starts_with("$argon2id$v=19$m=4096,t=2,p=1$"));
        let parsed = PasswordHash::new(&dummy).expect("parse dummy");
        assert!(cfg.hasher().verify_password(b"wrong", &parsed).is_err());

        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).expect("salt");
        let hashed = cfg
            .hasher()
            .hash_password(b"secret-pass", &salt)
            .expect("hash")
            .to_string();
        assert!(hashed.starts_with("$argon2id$v=19$m=4096,t=2,p=1$"));
        let parsed_real = PasswordHash::new(&hashed).expect("parse real");
        assert!(
            cfg.hasher()
                .verify_password(b"secret-pass", &parsed_real)
                .is_ok()
        );
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
