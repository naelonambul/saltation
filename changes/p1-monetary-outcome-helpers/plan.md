# Change Plan: `p1-monetary-outcome-helpers`

## Goal

Eliminate duplicated deadlock-retry, DB/ledger error-mapping, and idempotency-replay boilerplate across the 6 monetary feature modules (`transfers`, `inbound`, `deposits`, `loans`, `atm/operations`, `cards/payment`) by extracting cohesive shared helpers into `bank/src/shared/outcome/mod.rs`, while preserving every existing SQL statement (`oracle_tune_coverage.rs`), HTTP status/code contract, and `outcomes.rs` invariant.

## Order of Work

1. **Shared Monetary Outcome Helpers (`bank/src/shared/outcome/mod.rs`):**
   - `map_db_error(ctx: &RequestContext, feature: &'static str, error: &DbError) -> ApiError`: unified logging + `DbError::Busy | ORA-60 | ORA-30006 | ORA-1653 | ORA-1654 | ORA-1691` → `ApiError::busy(ctx)`, otherwise `ApiError::internal(ctx)`.
   - `ledger_error(error: LedgerError) -> DbError`: unified `LedgerError::Db(e) => e`, other variants → `DbError::Config(e.to_string())`.
   - `run_with_deadlock_retry<T, F>(db: &Db, stats: &RequestStats, mut op: F) -> Result<T, DbError>`: unified `ORA-00060` retry loop (up to 2 retries, `bank_deadlock_retries_total` counter, `10 * retries` ms backoff).
   - `StoredReplay` / `resolve_stored(...)`: unified `claimed_elsewhere` + stored request-hash comparison returning either a replayable stored row or `Conflict` (while each feature keeps its own `INSERT_REQUEST` / `SELECT_STORED` SQL statements untouched).
   - Add unit tests in `bank/src/shared/outcome/mod.rs` covering `map_db_error` (busy/lock/tablespace vs unexpected) and `resolve_stored` (matching hash replay, mismatched hash conflict, claimed elsewhere).

2. **Adopt Shared Helpers in All 6 Monetary Feature Modules (`transfers/mod.rs`, `inbound/mod.rs`, `deposits/mod.rs`, `loans/mod.rs`, `atm/operations.rs`, `cards/payment.rs`):**
   - Replace per-module `map_db_error`/`db_error`, `ledger_error`, and duplicated `ORA-00060` deadlock retry loops with `outcome::map_db_error`, `outcome::ledger_error`, and `outcome::run_with_deadlock_retry`.
   - Replace duplicated `claimed_elsewhere` + stored-row hash check branches with `outcome::resolve_stored` where applicable, without altering any HTTP status, `rsp_code`, `bank_rsp_code`, or outbox event payload.

## Verification

- Local:
  - `cargo fmt --check`
  - `cargo clippy --lib --bins --tests -- -D warnings`
  - `cargo test --lib`
  - `python3 scripts/repo.py status --change p1-monetary-outcome-helpers`
  - `python3 scripts/repo.py verify --change p1-monetary-outcome-helpers --group core`
- CI (PR):
  - Full `rust`, `control-plane`, `status`, `identity`, and `oracle` (including `outcomes.rs`, `transfers.rs`, `deposits`, `loans`, `atm.rs`, `cards`, `oracle_tune_coverage.rs`) green.
