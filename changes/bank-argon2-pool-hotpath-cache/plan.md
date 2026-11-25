# Implementation Plan

## Summary

Reduce `bank` CPU overhead by trading a small, bounded amount of RAM (~60 MiB out of `bank`'s 1 GiB limit) while keeping OWASP minimum Argon2id (`19456/2/1`) and every external API/spec/FDS invariant 100% unchanged:

1. **Pre-allocated Argon2id scratch buffer pool (`~57 MiB` RAM at `concurrency=3`):** Replace per-call 19 MiB `Vec<Block>` allocation (`mmap` → 4,864 minor page faults + kernel zero-fill → `munmap` + TLB shootdown under `GLIBC_TUNABLES=...mmap_threshold=131072`, which consumed ~20.1% of `bank` CPU in kernel `system_usec`) with a bounded pool of `concurrency` reusable `Vec<argon2::Block>` slots guarded by the existing `Semaphore`, calling the safe public `Argon2::hash_password_into_with_memory` API (`unsafe_code = "forbid"` preserved).
2. **Parallel seeding in `internal/mod.rs`:** Hash `seed_customers` and `seed_accounts` batches concurrently (bounded naturally by the Argon2id pool/semaphore) instead of sequentially awaiting one hash at a time, cutting seeding wall time by ~33%.
3. **Generation-guarded in-memory `SIM_CLOCK` state cache (`~128 B` RAM):** Cache the single-row `sim_clock` state inside `Clock` using `ClockCache { generation: u64, entry: Option<CachedClock> }`, eliminating one `spawn_blocking` + Oracle round-trip per HTTP request. Guard against stale write-back and mid-rollover poisoning by checking `observed_gen == guard.generation && self.barrier.try_read().is_ok()` before publishing, invalidating via an RAII drop guard on all write/rollover paths (`set_locked`, `go_live`, `catch_up`) and after commit before refreshing, using `state_uncached()` in `advance()`/`set()` under `barrier.write()`, re-checking `pending_rollover || overdue` under `barrier.write()` in `catch_up()`, and calling `pub fn invalidate_cache(&self)` from `bank/tests/admin.rs::clock_set`.
4. **Hot-path micro-optimizations & release profile:** Replace `auth::hex()`'s 32-allocation `format!` loop with a single-allocation stack lookup table; replace `cards/fx.rs`'s linear 249-string country scan with a 676-byte `O(1)` `[bool; 676]` table; add `[profile.release] lto = "thin"` and `codegen-units = 1` in `bank/Cargo.toml`.

*Note on `AuthClient` tokens:* An earlier draft considered caching 90-day counterparty `AuthClient` tokens in memory, but `bank/tests/outcomes.rs` mutates `access_token.scope` directly in SQL to verify immediate 403 enforcement. Keeping `AuthClient` lookups live against Oracle guarantees immediate scope/revocation accuracy and avoids security drift.

## Files and components that change

- `bank/Cargo.toml`: add `[profile.release]` (`lto = "thin"`, `codegen-units = 1`).
- `bank/src/shared/auth/mod.rs`: pre-allocated `Vec<argon2::Block>` scratch pool for `hash_secret` / `verify_secret` using `hash_password_into_with_memory` + constant-time `password_hash::Output` comparison; single-allocation lookup-table `hex()`.
- `bank/src/shared/clock/mod.rs`: generation-guarded `ClockCache` inside `Clock` with RAII `InvalidateOnDrop` on write/rollover paths, `barrier.try_read()` check before cache publish, `state_uncached()` in `advance()`/`set()`, midnight thundering-herd re-check in `catch_up()`, and `pub fn invalidate_cache(&self)`.
- `bank/src/features/internal/mod.rs`: parallelize Argon2id hashing in `seed_customers` and `seed_accounts` using `tokio::task::JoinSet` (preserving input order and bounding concurrency via `auth::hash_secret`'s pool).
- `bank/src/features/cards/fx.rs`: replace linear `Vec<String>` scan for ISO country codes with `[bool; 676]` `OnceLock` lookup table.
- `bank/tests/admin.rs`: call `api.state.clock.invalidate_cache()` after direct SQL `CLOCK_SET` in `clock_set()`.

## Order of work

1. Implement the Argon2id scratch pool (`hash_secret` / `verify_secret` via `hash_password_into_with_memory`) and lookup-table `hex()` in `bank/src/shared/auth/mod.rs`, with unit tests verifying byte-identical PHC format, cross-compatibility with stock `Argon2::verify_password`, variable-param fallback, and concurrent `Argon2Pool` calls exceeding `concurrency`.
2. Implement generation-guarded `ClockCache` + RAII `InvalidateOnDrop` + `catch_up()` re-check + `invalidate_cache()` in `bank/src/shared/clock/mod.rs` (with unit tests for generation-guarded stale write-back rejection and invalidation), and wire `api.state.clock.invalidate_cache()` into `bank/tests/admin.rs::clock_set`.
3. Parallelize `seed_customers` and `seed_accounts` hashing in `bank/src/features/internal/mod.rs`.
4. Replace `COUNTRIES` linear scan with `[bool; 676]` in `bank/src/features/cards/fx.rs`, and add `[profile.release]` (`lto = "thin"`, `codegen-units = 1`) in `bank/Cargo.toml`.
5. Run `repo.py status --change bank-argon2-pool-hotpath-cache`, `cargo fmt --check`, `cargo test --lib`, `cargo clippy --lib --bins --tests -- -D warnings`, and `repo.py verify --change bank-argon2-pool-hotpath-cache --group core`; validate full `rust` + `oracle` integration in GitHub Actions CI and record OTN-compliant before/after OCI benchmark results in the PR body.

## Tests and proof

- Unit tests in `bank/src/shared/auth/mod.rs`:
  - `hash_secret` + `verify_secret` round-trip and wrong-secret rejection using the pooled buffer.
  - Cross-verification: hash produced by pooled `hash_secret` verifies under stock `Argon2::verify_password`, and hash produced by stock `Argon2::hash_password` (including a hash with a different `m_kib`) verifies under pooled `verify_secret`.
  - Concurrent `Argon2Pool` test spawning more tasks than `concurrency` (`hash_with_memory` + `verify_with_memory` through `Argon2Pool`/`SlotGuard`) confirming zero deadlock, zero buffer corruption, and all slots returned.
  - `hex()` matches expected lowercase hex output for empty and boundary byte values.
- Unit tests in `bank/src/shared/clock/mod.rs`:
  - `ClockCache` generation guard rejects stale write-back after `invalidate()` and accepts matching-generation publish.
- Unit tests in `bank/src/features/cards/fx.rs`: existing country validation tests pass with `[bool; 676]`.
- `python3 scripts/repo.py status --change bank-argon2-pool-hotpath-cache` and `repo.py verify --change bank-argon2-pool-hotpath-cache --group core` pass locally, and full `rust` + `oracle` integration CI passes on OCI.

## Risks and mitigations

- **Risk:** A stored PHC hash in tests or after a future parameter migration has a different `m_kib` than the pooled buffer size.
  - **Mitigation:** Check parsed `m_cost` against `buf.len()`; if it differs, fall back to a one-off `vec![Block::new(); blocks]` for that call instead of panicking or failing verification.
- **Risk:** `Clock` cache becomes stale or poisoned by an in-flight unguarded `state_uncached()` racing with a mutation/rollover or a failed `roll_to`.
  - **Mitigation:** Track a monotonic `generation` counter in `ClockCache`, require `observed_gen == current_gen && barrier.try_read().is_ok()` to publish into cache, attach an RAII `InvalidateOnDrop` guard to every write/rollover path (`set_locked`, `go_live`, `catch_up`), invalidate after commit before refreshing, and call `invalidate_cache()` in `bank/tests/admin.rs::clock_set`.

## Rollback or recovery

Revert the PR commit; no database schema, API contract, or Argon2id PHC format changes are introduced.

## Open questions

None.
