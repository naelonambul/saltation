# Implementation Plan

## Summary

Reduce `bank` CPU overhead by trading a small, bounded amount of RAM (~60 MiB out of `bank`'s 1 GiB limit) while keeping OWASP minimum Argon2id (`19456/2/1`) and every external API/spec/FDS invariant 100% unchanged:

1. **Pre-allocated Argon2id scratch buffer pool (`~57 MiB` RAM at `concurrency=3`):** Replace per-call 19 MiB `Vec<Block>` allocation (`mmap` → 4,864 minor page faults + kernel zero-fill → `munmap` + TLB shootdown under `GLIBC_TUNABLES=...mmap_threshold=131072`, which consumed ~20.1% of `bank` CPU in kernel `system_usec`) with a bounded pool of `concurrency` reusable `Vec<argon2::Block>` slots guarded by the existing `Semaphore`, calling the safe public `Argon2::hash_password_into_with_memory` API (`unsafe_code = "forbid"` preserved).
2. **Parallel seeding in `internal/mod.rs`:** Hash `seed_customers` and `seed_accounts` batches concurrently (bounded naturally by the Argon2id pool/semaphore) instead of sequentially awaiting one hash at a time, cutting seeding wall time by ~2–3×.
3. **In-memory `SIM_CLOCK` state cache (`~128 B` RAM):** Cache the single-row `sim_clock` state inside `Clock` (already serialized through `self.barrier: Arc<RwLock<()>>` on all mutations), eliminating one `spawn_blocking` + Oracle round-trip per HTTP request while refreshing/invalidating on every `set`, `advance`, `go_live`, `catch_up`, and `roll_to` failure (plus a test helper `invalidate_cache()` called from `bank/tests/admin.rs::clock_set`).
4. **In-memory `AuthClient` bearer token cache (`<64 KiB` RAM on `AppState`):** Cache unrevoked 90-day counterparty client tokens (`token_hash -> (AuthClient, expires_at)`) with `expires_at > now` check against simulated `BusinessTime(now)`, eliminating repeated `SELECT_CLIENT_TOKEN` queries on card/inbound paths (~27% of all requests).
5. **Hot-path micro-optimizations & release profile:** Replace `auth::hex()`'s 32-allocation `format!` loop with a single-allocation stack lookup table; replace `cards/fx.rs`'s linear 249-string country scan with a 676-byte `O(1)` `[bool; 676]` table; add `[profile.release] lto = "thin"` and `codegen-units = 1` in `bank/Cargo.toml`.

## Files and components that change

- `bank/Cargo.toml`: add `[profile.release]` (`lto = "thin"`, `codegen-units = 1`).
- `bank/src/shared/auth/mod.rs`: pre-allocated `Vec<argon2::Block>` scratch pool for `hash_secret` / `verify_secret` using `hash_password_into_with_memory` + constant-time `password_hash::Output` comparison; single-allocation lookup-table `hex()`; `ClientTokenCache` struct and lookup/insert in `AuthClient::from_request_parts`.
- `bank/src/shared/auth/sql/select_client_token.sql`: include `t.expires_at` in `SELECT` list so cached `AuthClient` entries enforce `expires_at > now` in memory.
- `bank/src/app.rs`: attach `ClientTokenCache` to `AppState` (per-instance, clean isolation across tests).
- `bank/src/shared/clock/mod.rs`: cache `sim_clock` row inside `Clock` behind `RwLock<Option<CachedClock>>`, refreshed/invalidated on all write paths and errors, plus `pub async fn invalidate_cache(&self)`.
- `bank/src/features/internal/mod.rs`: parallelize Argon2id hashing in `seed_customers` and `seed_accounts` using `tokio::task::JoinSet` (preserving input order and bounding concurrency via `auth::hash_secret`'s pool).
- `bank/src/features/cards/fx.rs`: replace linear `Vec<String>` scan for ISO country codes with `[bool; 676]` `OnceLock` lookup table.
- `bank/tests/admin.rs`: call `api.state.clock.invalidate_cache().await` after direct SQL `CLOCK_SET` in `clock_set()`.

## Order of work

1. Implement the Argon2id scratch pool (`hash_secret` / `verify_secret` via `hash_password_into_with_memory`) and lookup-table `hex()` in `bank/src/shared/auth/mod.rs`, with unit tests verifying byte-identical PHC format, cross-compatibility with standard `Argon2::verify_password`, variable-param verification (if a stored hash has different `m_kib` than the active pool, safely fall back to on-demand allocation), and constant-time rejection.
2. Add `ClientTokenCache` to `bank/src/shared/auth/mod.rs`, `bank/src/shared/auth/sql/select_client_token.sql`, and `bank/src/app.rs`, with unit tests covering hit, expiry against simulated `BusinessTime`, and bounded capacity eviction.
3. Add in-memory `Clock` caching and `invalidate_cache()` in `bank/src/shared/clock/mod.rs` and wire `api.state.clock.invalidate_cache().await` into `bank/tests/admin.rs::clock_set`.
4. Parallelize `seed_customers` and `seed_accounts` hashing in `bank/src/features/internal/mod.rs`.
5. Replace `COUNTRIES` linear scan with `[bool; 676]` in `bank/src/features/cards/fx.rs`, and add `[profile.release]` (`lto = "thin"`, `codegen-units = 1`) in `bank/Cargo.toml`.
6. Run `repo.py status --change bank-argon2-pool-hotpath-cache` and `repo.py verify --change bank-argon2-pool-hotpath-cache` (including `rust-fmt`, `rust-clippy`, `rust-unit`, and `control-plane`).

## Tests and proof

- Unit tests in `bank/src/shared/auth/mod.rs`:
  - `hash_secret` + `verify_secret` round-trip and wrong-secret rejection using the pooled buffer.
  - Cross-verification: hash produced by pooled `hash_secret` verifies under stock `Argon2::verify_password`, and hash produced by stock `Argon2::hash_password` (including a hash with a different `m_kib`) verifies under pooled `verify_secret`.
  - Concurrent `hash_secret`/`verify_secret` calls exceeding pool size (`concurrency`) succeed cleanly without deadlock or buffer corruption.
  - `hex()` matches expected lowercase hex output for empty, 32-byte SHA-256, and boundary byte values.
  - `ClientTokenCache` returns cached entries before `expires_at` and rejects/evicts once `now >= expires_at`.
- Unit tests in `bank/src/features/cards/fx.rs`: existing country validation tests pass with `[bool; 676]`.
- `python3 scripts/repo.py verify --change bank-argon2-pool-hotpath-cache` passes (`rust-fmt`, `rust-clippy`, `rust-unit`, `control-plane`), and full OCI `oracle-integration` CI validates live `Clock` + `AuthClient` + `seed_customers`/`seed_accounts` + `admin.rs` + `backfill.rs` end-to-end.

## Risks and mitigations

- **Risk:** A stored PHC hash in tests or after a future parameter migration has a different `m_kib` than the pooled buffer size.
  - **Mitigation:** Check parsed `m_cost` against `buf.len()`; if it differs, fall back to a one-off `vec![Block::new(); blocks]` for that call instead of panicking or failing verification.
- **Risk:** `Clock` cache becomes stale if a rollover fails partway (`pending_rollover = true`) or a test updates `sim_clock` directly in SQL.
  - **Mitigation:** Always invalidate the cache before and after write operations (and on any error in `roll_to` / `set_locked` / `go_live` / `catch_up`), and call `invalidate_cache()` in `bank/tests/admin.rs::clock_set`.
- **Risk:** `ClientTokenCache` grows unbounded or serves expired tokens when simulated clock jumps forward.
  - **Mitigation:** Bound cache size (e.g., 1,024 entries; clear/evict on cap) and check `entry.expires_at > now` on every lookup using the request's `BusinessTime(now)`.

## Rollback or recovery

Revert the PR commit; no database schema, API contract, or Argon2id PHC format changes are introduced.

## Open questions

None.
