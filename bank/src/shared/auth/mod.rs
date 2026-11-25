//! Authentication primitives (spec F-2, F-3, N-6): Argon2id password hashing
//! behind its own small concurrency gate, opaque bearer tokens stored as
//! SHA-256 hashes, and the customer bearer-token extractor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use argon2::password_hash::{Output, ParamsString, PasswordHash, SaltString};
use argon2::{Algorithm, Argon2, Block, Params, Version};
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

    fn params(self) -> Params {
        Params::new(self.m_kib, self.t_cost, self.p_cost, None).expect("validated Argon2")
    }

    #[cfg(test)]
    fn hasher(self) -> Argon2<'static> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.params())
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

struct Argon2Pool {
    gate: Arc<Semaphore>,
    slots: Arc<Mutex<Vec<Vec<Block>>>>,
    block_count: usize,
}

impl Argon2Pool {
    fn new(cfg: Argon2Config, preallocate: bool) -> Self {
        let block_count = cfg.params().block_count();
        let mut slots = Vec::with_capacity(cfg.concurrency);
        if preallocate {
            for _ in 0..cfg.concurrency {
                slots.push(vec![Block::new(); block_count]);
            }
        }
        Self {
            gate: Arc::new(Semaphore::new(cfg.concurrency)),
            slots: Arc::new(Mutex::new(slots)),
            block_count,
        }
    }

    fn checkout(&self) -> SlotGuard {
        let buf = self
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .unwrap_or_else(|| vec![Block::new(); self.block_count]);
        SlotGuard {
            buf: Some(buf),
            slots: Arc::clone(&self.slots),
        }
    }
}

struct SlotGuard {
    buf: Option<Vec<Block>>,
    slots: Arc<Mutex<Vec<Vec<Block>>>>,
}

impl SlotGuard {
    fn as_mut_slice(&mut self) -> &mut [Block] {
        self.buf
            .as_mut()
            .expect("slot buffer present")
            .as_mut_slice()
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.slots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(buf);
        }
    }
}

static ARGON2_CONFIG: OnceLock<Argon2Config> = OnceLock::new();
static ARGON2_POOL: OnceLock<Argon2Pool> = OnceLock::new();
static DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// Validates and locks Argon2id parameters from the environment at process startup,
/// pre-allocating reusable scratch buffers (one per concurrency slot).
pub fn init_argon2_from_env() -> anyhow::Result<Argon2Config> {
    let cfg = Argon2Config::from_env()?;
    let active = *ARGON2_CONFIG.get_or_init(|| cfg);
    let _ = ARGON2_POOL.get_or_init(|| Argon2Pool::new(active, true));
    let _ = DUMMY_HASH.get_or_init(|| active.dummy_phc());
    Ok(active)
}

fn config() -> Argon2Config {
    *ARGON2_CONFIG.get_or_init(|| Argon2Config::from_env().unwrap_or_default())
}

fn pool() -> &'static Argon2Pool {
    ARGON2_POOL.get_or_init(|| Argon2Pool::new(config(), false))
}

/// Valid Argon2id PHC hash matching the active N-6 parameters for constant-work rejection paths.
pub fn dummy_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| config().dummy_phc()).as_str()
}

fn hash_with_memory(
    cfg: Argon2Config,
    secret: &[u8],
    memory: &mut [Block],
) -> anyhow::Result<String> {
    let mut salt_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt_str = SaltString::encode_b64(&salt_bytes).map_err(|e| anyhow::anyhow!("salt: {e}"))?;
    let params = cfg.params();
    let needed = params.block_count();
    let params_string =
        ParamsString::try_from(&params).map_err(|e| anyhow::anyhow!("params: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; Params::DEFAULT_OUTPUT_LEN];
    if needed <= memory.len() {
        argon2
            .hash_password_into_with_memory(secret, &salt_bytes, &mut out, &mut memory[..needed])
            .map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    } else {
        let mut fallback = vec![Block::new(); needed];
        argon2
            .hash_password_into_with_memory(secret, &salt_bytes, &mut out, &mut fallback)
            .map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    }
    let output = Output::new(&out).map_err(|e| anyhow::anyhow!("output: {e}"))?;
    let phc = PasswordHash {
        algorithm: Algorithm::Argon2id.ident(),
        version: Some(Version::V0x13.into()),
        params: params_string,
        salt: Some(salt_str.as_salt()),
        hash: Some(output),
    };
    Ok(phc.to_string())
}

fn verify_with_memory(hash: &str, secret: &[u8], memory: &mut [Block]) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    let Ok(algorithm) = Algorithm::try_from(parsed.algorithm) else {
        return Ok(false);
    };
    let version = match parsed.version {
        Some(v) => match Version::try_from(v) {
            Ok(ver) => ver,
            Err(_) => return Ok(false),
        },
        None => Version::V0x13,
    };
    let Ok(params) = Params::try_from(&parsed) else {
        return Ok(false);
    };
    let Some(salt) = parsed.salt else {
        return Ok(false);
    };
    let Some(expected) = parsed.hash else {
        return Ok(false);
    };
    let mut salt_buf = [0u8; 64];
    let Ok(salt_bytes) = salt.decode_b64(&mut salt_buf) else {
        return Ok(false);
    };
    let needed = params.block_count();
    let argon2 = Argon2::new(algorithm, version, params);
    let mut out_buf = [0u8; Output::MAX_LENGTH];
    let out = &mut out_buf[..expected.len()];
    let res = if needed <= memory.len() {
        argon2.hash_password_into_with_memory(secret, salt_bytes, out, &mut memory[..needed])
    } else {
        let mut fallback = vec![Block::new(); needed];
        argon2.hash_password_into_with_memory(secret, salt_bytes, out, &mut fallback)
    };
    if res.is_err() {
        return Ok(false);
    }
    let Ok(computed) = Output::new(out) else {
        return Ok(false);
    };
    Ok(computed == expected)
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

/// Hashes a secret (password or PIN) with Argon2id on a blocking thread using a pooled buffer.
pub async fn hash_secret(secret: String) -> anyhow::Result<String> {
    let p = pool();
    let cfg = config();
    run_gated_blocking(&p.gate, "hash", move || {
        let mut slot = p.checkout();
        hash_with_memory(cfg, secret.as_bytes(), slot.as_mut_slice())
    })
    .await
}

/// Verifies a secret against a stored Argon2id hash on a blocking thread using a pooled buffer.
pub async fn verify_secret(hash: String, secret: String) -> anyhow::Result<bool> {
    let p = pool();
    run_gated_blocking(&p.gate, "verify", move || {
        let mut slot = p.checkout();
        verify_with_memory(&hash, secret.as_bytes(), slot.as_mut_slice())
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
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0x0f) as usize] as char);
    }
    out
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
        use argon2::password_hash::{PasswordHasher, PasswordVerifier};

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

    #[test]
    fn pooled_argon2_cross_verifies_with_stock_and_handles_mismatched_m_cost() {
        use argon2::password_hash::{PasswordHasher, PasswordVerifier};

        let small = Argon2Config::new(4096, 2, 1, 2).expect("small");
        let larger = Argon2Config::new(8192, 1, 1, 1).expect("larger");
        let mut buf = vec![Block::new(); small.params().block_count()];

        let pooled_phc = hash_with_memory(small, b"hunter2", &mut buf).expect("pooled hash");
        assert!(pooled_phc.starts_with("$argon2id$v=19$m=4096,t=2,p=1$"));

        // Pooled hash verifies under stock PasswordVerifier and under verify_with_memory.
        let parsed = PasswordHash::new(&pooled_phc).expect("parse pooled");
        assert!(small.hasher().verify_password(b"hunter2", &parsed).is_ok());
        assert!(verify_with_memory(&pooled_phc, b"hunter2", &mut buf).expect("verify ok"));
        assert!(!verify_with_memory(&pooled_phc, b"wrong", &mut buf).expect("verify wrong"));

        // Stock hash with larger m_cost than pool buffer falls back cleanly.
        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).expect("salt");
        let stock_larger = larger
            .hasher()
            .hash_password(b"bigger-secret", &salt)
            .expect("stock hash")
            .to_string();
        assert!(verify_with_memory(&stock_larger, b"bigger-secret", &mut buf).expect("fallback"));
        assert!(!verify_with_memory(&stock_larger, b"nope", &mut buf).expect("fallback wrong"));
    }

    #[test]
    fn hex_lookup_table_matches_lowercase_hex() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x00, 0x09, 0x0a, 0x1f, 0xab, 0xff]), "00090a1fabff");
    }

    #[test]
    fn client_token_cache_honors_business_time_expiry_and_bound() {
        let cache = ClientTokenCache::new(2);
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-03-01T12:00:00+09:00").expect("t0");
        let exp = t0 + Duration::minutes(30);
        let c = AuthClient {
            counterparty_client_id: 1,
            client_id: "card-net".into(),
            institution_code: "001".into(),
            scope: "card".into(),
        };
        cache.insert("h1".into(), c.clone(), exp);
        assert_eq!(cache.get("h1", t0).expect("hit").client_id, "card-net");
        assert!(cache.get("h1", exp).is_none(), "expired at boundary");

        cache.insert("h2".into(), c.clone(), exp);
        cache.insert("h3".into(), c, exp);
        assert!(cache.get("h3", t0).is_some());
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

const CLIENT_TOKEN_CACHE_CAP: usize = 1024;

#[derive(Clone)]
struct CachedClientToken {
    client: AuthClient,
    expires_at: chrono::DateTime<chrono::FixedOffset>,
}

struct ClientTokenCache {
    cap: usize,
    entries: RwLock<HashMap<String, CachedClientToken>>,
}

impl ClientTokenCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn get(&self, hash: &str, now: chrono::DateTime<chrono::FixedOffset>) -> Option<AuthClient> {
        let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
        let entry = map.get(hash)?;
        if entry.expires_at > now {
            Some(entry.client.clone())
        } else {
            None
        }
    }

    fn insert(
        &self,
        hash: String,
        client: AuthClient,
        expires_at: chrono::DateTime<chrono::FixedOffset>,
    ) {
        let mut map = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.cap && !map.contains_key(&hash) {
            map.clear();
        }
        map.insert(hash, CachedClientToken { client, expires_at });
    }
}

static CLIENT_TOKEN_CACHE: OnceLock<ClientTokenCache> = OnceLock::new();

fn client_token_cache() -> &'static ClientTokenCache {
    CLIENT_TOKEN_CACHE.get_or_init(|| ClientTokenCache::new(CLIENT_TOKEN_CACHE_CAP))
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
        let client = if let Some(cached) = client_token_cache().get(&hash, now) {
            cached
        } else {
            let lookup_hash = hash.clone();
            let row: Option<(
                i64,
                String,
                String,
                String,
                chrono::DateTime<chrono::FixedOffset>,
            )> = state
                .db
                .run_in(&stats, move |s| {
                    s.query_opt(&SELECT_CLIENT_TOKEN, &[&lookup_hash, &now])
                })
                .await
                .map_err(|e| {
                    if matches!(e, crate::shared::db::DbError::Busy) {
                        ApiError::busy(&ctx)
                    } else {
                        ApiError::internal(&ctx)
                    }
                })?;
            let (counterparty_client_id, client_id, institution_code, scope, expires_at) =
                row.ok_or_else(unauthorized)?;
            let client = Self {
                counterparty_client_id,
                client_id,
                institution_code,
                scope,
            };
            client_token_cache().insert(hash, client.clone(), expires_at);
            client
        };
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
