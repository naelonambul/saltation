# Change Plan: `bank-p1-hardening`

## Goal

Fix three P1 correctness, security, and lock-contention hardening items in `bank/` identified during the maintainer review without changing any external API contract or specification behavior:

1. **Outbox error classification and centralized `bank_outbox_full_total` metric (`bank/src/shared/outbox/mod.rs`, `bank/src/features/transfers/mod.rs`):**
   - Stop masking all `INSERT INTO outbox` errors as `DbError::Busy`.
   - Classify outbox insert errors via a dedicated helper:
     - `ORA-1653 | ORA-1654 | ORA-1691` (tablespace full): increment `bank_outbox_full_total` centrally, log an error, and return `DbError::Busy` (503 `A0013` per F-36a).
     - `ORA-60 | ORA-30006` or `DbError::Busy`: log a warning and return `DbError::Busy`.
     - Any other error (e.g., `ORA-12899`, `ORA-01400`, `ORA-00001`): log an error and propagate the original `DbError` (500) instead of masking it as retryable `503`.
   - Remove the unreachable `bank_outbox_full_total` increment from `bank/src/features/transfers/mod.rs` while keeping its `30006 | 1653 | 1654 | 1691` -> `ApiError::busy` mapping.
   - Add unit tests in `bank/src/shared/outbox/mod.rs` covering all three classifications.

2. **Avoid unauthenticated second Argon2id hash in `password_reset::confirm` (`bank/src/features/password_reset/mod.rs`):**
   - Currently `confirm()` always runs a second 19 MiB OWASP Argon2id `hash_secret(req.new_password)` even when the 6-digit reset code failed verification or the phone does not exist.
   - Keep the constant-work first Argon2id step (`verify_secret` when a code exists, `hash_secret(req.code)` when absent) so unknown-phone and wrong-code paths stay timing-equivalent.
   - Only hash `req.new_password` when `verified == true`, eliminating the 2x Argon2id DoS amplification on unauthenticated failed attempts.

3. **Consistent `FOR UPDATE WAIT 1` and `ORA-30006 / ORA-60 -> 503` mapping (`auth`, `atm`, `password_reset`):**
   - Add `WAIT 1` to the remaining unbounded `FOR UPDATE` queries:
     - `bank/src/features/auth/sql/lock_credential.sql`
     - `bank/src/features/atm/sql/lock_terminal.sql`
     - `bank/src/features/atm/sql/lock_credential.sql` (`FOR UPDATE OF c.account_no WAIT 1`)
     - `bank/src/features/atm/sql/lock_session.sql`
     - `bank/src/features/atm/sql/open_session.sql`
     - `bank/src/features/password_reset/sql/select_customer.sql`
     - `bank/src/features/password_reset/sql/select_code.sql`
   - Update `db_error()` in `bank/src/features/auth/mod.rs`, `bank/src/features/atm/mod.rs`, and `bank/src/features/password_reset/mod.rs` to map `Some(60 | 30006)` to `ApiError::busy(ctx)` (`503 A0013`), matching `transfers`, `deposits`, `loans`, `inbound`, `cards`, and `admin`.

## Verification

- Local:
  - `cargo fmt --check`
  - `cargo clippy --lib --bins --tests -- -D warnings`
  - `cargo test --lib`
  - `python3 scripts/repo.py status --change bank-p1-hardening`
  - `python3 scripts/repo.py verify --change bank-p1-hardening --group core`
- CI (PR):
  - Full `rust`, `control-plane`, `status`, `identity`, and `oracle` (including `oracle_tune_coverage`, `auth`, `password_reset`, `atm`, `outcomes`, `generator_scenarios`, `generator_lifecycle`) groups green.
