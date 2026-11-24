# Make Argon2id N-6 parameters configurable and add auth timing metrics

## Summary

Implement spec N-6 ("password and PIN hashing uses Argon2id with parameters fixed in configuration and sized so that login stays within N-4") and add auth timing instrumentation so OCI ARM64 calibration can directly measure password-hashing cost vs gate wait without modifying existing defaults yet.

1. **Configurable Argon2id parameters & gate concurrency (`bank/src/shared/auth/mod.rs`, `bank/src/main.rs`, `infra/compose/compose.yml`):**
   - Read and validate `SALTATION_ARGON2_M_KIB` (default `19456`, range `4096..=65536`), `SALTATION_ARGON2_T_COST` (default `2`, range `1..=10`), `SALTATION_ARGON2_P_COST` (default `1`, range `1..=4`), and `SALTATION_ARGON2_CONCURRENCY` (default `2`, range `1..=8`) at startup.
   - Keep existing defaults (`19456 / 2 / 1`, concurrency `2`) unchanged in this packet so behavior is preserved until OCI sizing measurements select the final N-6/N-7 values.
2. **Dynamic dummy hash (`bank/src/shared/auth/mod.rs`, `atm/mod.rs`, `admin/login.rs`, `inbound/mod.rs`):**
   - Replace hardcoded `m=19456,t=2,p=1` `DUMMY_HASH` constants with `auth::dummy_hash()` generated from the active `Argon2Config` so unknown-account/client probes match configured timing and cost.
3. **Auth timing instrumentation (`bank/src/shared/auth/mod.rs`):**
   - Export `bank_auth_gate_wait_seconds` (semaphore wait) and `bank_auth_hash_duration_seconds` (`op="hash"|"verify"`, blocking closure wall time) to isolate Argon2id cost from DB/HTTP latency during OCI sizing sweeps.

## Files and components that change

| Paths | Work |
| --- | --- |
| `bank/src/shared/auth/mod.rs` | Add validated `Argon2Config`, `init_argon2_from_env()`, `dummy_hash()`, `bank_auth_gate_wait_seconds` and `bank_auth_hash_duration_seconds` histograms, and unit tests for config validation, PHC encoding, and dummy hash parity. |
| `bank/src/main.rs` | Call `bank::shared::auth::init_argon2_from_env()?` at startup and log the active `(m_kib, t_cost, p_cost, concurrency)` parameters. |
| `bank/src/features/atm/mod.rs`, `bank/src/features/admin/login.rs`, `bank/src/features/inbound/mod.rs` | Replace hardcoded `DUMMY_HASH` constants with `auth::dummy_hash()`. |
| `infra/compose/compose.yml` | Pass `SALTATION_ARGON2_M_KIB`, `SALTATION_ARGON2_T_COST`, `SALTATION_ARGON2_P_COST`, and `SALTATION_ARGON2_CONCURRENCY` (with current defaults) into the `bank` service. |

## Order of work

1. **Add configurable `Argon2Config`, `dummy_hash()`, and auth metrics in `bank/src/shared/auth/mod.rs` and `bank/src/main.rs`.** Preserve `OwnedSemaphorePermit` inside `spawn_blocking`. Add unit tests covering bounds validation, custom `(m, t, p)` PHC round-trips, and `dummy_hash()` parameter matching. Proof: `cargo test --manifest-path bank/Cargo.toml --locked --lib shared::auth`.
2. **Wire `auth::dummy_hash()` into `atm`, `admin`, and `inbound`.** Remove the three hardcoded `DUMMY_HASH` constants. Proof: `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`.
3. **Expose env passthrough in `infra/compose/compose.yml`.** Add the four `SALTATION_ARGON2_*` env vars with current defaults (`19456/2/1/2`). Proof: `PYTHONDONTWRITEBYTECODE=1 python3 scripts/ops/check_compose_budget.py static` and `python3 scripts/repo.py status --change bank-argon2-n6-config`.

## Tests and proof

- `cargo fmt --manifest-path bank/Cargo.toml --check`
- `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`
- `cargo test --manifest-path bank/Cargo.toml --locked --lib`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/ops/check_compose_budget.py static`
- `python3 scripts/repo.py status --change bank-argon2-n6-config`
- Full CI (`rust`, `python`, `oracle`, `control-plane`, `status`, `identity`, `summary`).

## Risks and mitigations

- **Existing stored hashes:** `argon2::PasswordVerifier` reads `(m, t, p)` from each stored PHC string, so changing env vars only affects newly hashed secrets (freshly seeded worlds); existing hashes continue to verify cleanly.
- **Invalid config at startup:** `init_argon2_from_env()` fails fast at process startup on out-of-range or unparseable values rather than silently falling back.

## Rollback or recovery

Revert the scoped files; defaults are identical to the prior hardcoded values.
