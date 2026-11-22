# Fix concurrent expired-lock login race and transfer idempotency hash delimiter collision

## Summary

Resolve two correctness/security bugs in `bank` identified during independent repository review:

1. **Expired-lock concurrent login race (F-3):** `login()` currently reads `locked_until` before Argon2 verification without `FOR UPDATE`, then unconditionally executes `RESET_FAILURES` inside the write transaction whenever the pre-Argon2 snapshot saw `locked_until <= now`. Concurrent wrong-password logins queued behind the 2-permit Argon2 gate all observe the stale expired lock, each reset `failed_count` to `0` before incrementing to `1`, and defeat the 5-failure lockout. In addition, `Outcome::Success` locks `customer_credential` via `LOCK_CREDENTIAL` without reading `locked_until`, allowing a concurrent correct-password login to bypass a lock acquired while Argon2 was running.
2. **Transfer idempotency hash delimiter collision (F-18):** `WithdrawRequest::hash()` joins 8 fields with literal `|`, including unrestricted string fields (`recv_client_name`, `wd_print_content`, `dps_print_content`). Shifting a `|` across adjacent fields produces an identical SHA-256 digest and causes F-18 to replay `200` instead of returning `409 Conflict`.

## Files and components that change

| Paths | Work |
| --- | --- |
| `bank/src/features/auth/sql/lock_credential.sql` | Return `password_hash, failed_count, locked_until` under `FOR UPDATE`. |
| `bank/src/features/auth/mod.rs` | Lock the credential row inside the write transaction for both `Outcome::WrongPassword` and `Outcome::Success`; reject as `Outcome::Locked` if the locked row currently has `locked_until > now`; reset expired locks only when the locked row itself still has `locked_until <= now`; propagate the effective outcome to `LOGIN_ATTEMPT`, outbox event, completion record, and HTTP response. |
| `bank/tests/auth.rs` | Add concurrent Oracle integration tests covering (a) concurrent wrong-password logins after lock expiry reaching 5 failures and re-locking, and (b) a concurrent valid-password login observing a freshly acquired lock instead of issuing a token. |
| `bank/src/features/transfers/mod.rs` | Replace `|`-delimited `WithdrawRequest::hash()` with structured JSON array serialization (`serde_json::json!([...]).to_string()`) matching other idempotent endpoints, and add unit tests covering delimiter-shift collisions. |

## Order of work

1. **Fix concurrent login lock handling in `bank/src/features/auth/`.** Update `sql/lock_credential.sql` to select `password_hash, failed_count, locked_until ... FOR UPDATE`. Update `login()` in `bank/src/features/auth/mod.rs` so both `WrongPassword` and `Success` acquire `LOCK_CREDENTIAL`, check `current_locked > now` to downgrade to `Outcome::Locked` (preserving `current_failed` without mutating the credential or issuing a session/token), and reset expired locks only when `current_locked <= now` under the row lock. Proof: `cargo test --manifest-path bank/Cargo.toml --locked --lib`.
2. **Fix transfer idempotency hash canonicalization in `bank/src/features/transfers/mod.rs`.** Replace `format!("{}|...", ...)` in `WithdrawRequest::hash()` with `json!([...]).to_string()` and add unit tests asserting that delimiter-shifted fields (e.g. `"ALPHA|BETA"` + `"memo"` vs `"ALPHA"` + `"BETA|memo"`) produce distinct hashes while identical payloads remain deterministic. Proof: `cargo test --manifest-path bank/Cargo.toml --locked --lib features::transfers`.
3. **Add concurrent login integration tests in `bank/tests/auth.rs`.** Add Oracle-backed integration tests for concurrent wrong-password bursts after lock expiry (verifying `failed_count` reaches 5 and locks) and a valid login racing with a lock. Proof: `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`, `cargo fmt --manifest-path bank/Cargo.toml --check`, and full CI `oracle` group (`scripts/dev/it-oracle.sh`).

## Tests and proof

- Rust formatting & lint: `cargo fmt --manifest-path bank/Cargo.toml --check`, `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`.
- Rust unit tests: `cargo test --manifest-path bank/Cargo.toml --locked --lib` (including new transfer hash collision regression tests).
- Oracle integration tests (CI `oracle` group): `bank/tests/auth.rs` concurrent expired-lock and lock-vs-success tests, plus existing transfer idempotency tests in `bank/tests/transfers.rs`.
- Control plane: `python3 scripts/repo.py status --change bank-auth-transfer-hardening`.

## Risks and mitigations

- **In-flight replay compatibility for transfer hashes:** Changing `WithdrawRequest::hash()` changes the hash format for future requests. Any previously stored `IDEMPOTENCY` rows from older runs are keyed by client `request_id` (`X-Request-Id`), and `saltation` is a development/simulation system whose test and backfill runs start with fresh request IDs.
- **Row-lock serialization on concurrent logins for the same customer:** Acquiring `FOR UPDATE` inside the short write transaction (after Argon2 completes) serializes only the DB update for the same `customer_id`, keeping Argon2 CPU work outside the DB transaction.

## Rollback or recovery

Revert the scoped changes in `bank/src/features/auth/`, `bank/src/features/transfers/mod.rs`, and `bank/tests/auth.rs`. No schema migration is required.
