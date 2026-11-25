# Implementation Plan

## Summary

Batch per-row transactions inside the three rollover jobs (`DormancyJob`, `MaturityJob`, `AtmSessionExpiryJob`) that run under `clock.barrier.write()` at business-date boundaries (`P0-6` from the maintainer review). Currently each job executes one `db.run` checkout and one `s.commit()` per row, which can hold the exclusive clock barrier for tens of seconds on heavy dormancy/maturity days during a 365-day backfill or live midnight cutover.

Batching each job into bounded chunks (`200` for dormancy and ATM expiry, `100` for deposit maturity) reduces DB session checkouts and `COMMIT` round-trips by up to `100–200×` while keeping undo/redo per transaction small and preserving identical SQL statements, row-level eligibility checks, ledger movements, and outbox events.

## Files and components that change

- `bank/src/features/accounts_lifecycle/mod.rs`: process `DormancyJob` candidate `ids` in chunks of `200` per `db.run` + `s.commit()`.
- `bank/src/features/deposits/mod.rs`: process `MaturityJob` candidate `rows` in chunks of `100` per `db.run` + `s.commit()`.
- `bank/src/features/atm/mod.rs`: process `AtmSessionExpiryJob` candidate `rows` in chunks of `200` per `db.run` + `s.commit()`, caching `TERMINAL` metadata in a local `HashMap` across the batch.

## Order of work

1. Batch `DormancyJob::run` in `bank/src/features/accounts_lifecycle/mod.rs` (`const ROLLOVER_BATCH_SIZE: usize = 200`).
2. Batch `MaturityJob::run` in `bank/src/features/deposits/mod.rs` (`const MATURITY_BATCH_SIZE: usize = 100`).
3. Batch `AtmSessionExpiryJob::run` in `bank/src/features/atm/mod.rs` (`const EXPIRY_BATCH_SIZE: usize = 200`) with a local `HashMap<String, Terminal>` cache inside each batch closure.
4. Run `cargo fmt --check`, `cargo test --lib`, `cargo clippy --lib --bins --tests -- -D warnings`, `repo.py status --change bank-rollover-batching`, and `repo.py verify --change bank-rollover-batching --group core`; validate full OCI `oracle-integration` CI (covering `backfill.rs`, `deposits`, `atm`, and `stage6_lifecycle_harness.py`).

## Tests and proof

- `cargo fmt --manifest-path bank/Cargo.toml -- --check` passes.
- `cargo test --manifest-path bank/Cargo.toml --lib` passes (`63+` unit tests).
- `cargo clippy --manifest-path bank/Cargo.toml --lib --bins --tests -- -D warnings` passes with 0 warnings.
- `python3 scripts/repo.py status --change bank-rollover-batching` and `repo.py verify --change bank-rollover-batching --group core` pass locally.
- Full `rust` + `oracle` integration CI on OCI exercises `DormancyJob`, `MaturityJob`, and `AtmSessionExpiryJob` end-to-end.

## Risks and mitigations

- **Risk:** Partial failure inside a batch rolls back the entire chunk instead of only one row.
  - **Mitigation:** All three rollover jobs are already idempotent (`ELIGIBLE` re-check, `status != "CLOSED"` re-check, `EXPIRE` `WHERE ended_at IS NULL` returning `1` row) and `rollover_run` only marks a job finished after all chunks commit, so a retry after any transient failure cleanly processes remaining rows without duplicates.
- **Risk:** Lock contention within a batch.
  - **Mitigation:** All `RolloverJob`s run exclusively under `clock.barrier.write()` with zero concurrent request traffic, and chunk sizes (`100–200`) keep transaction undo/redo well within Oracle limits.

## Rollback or recovery

Revert the commit; no schema, SQL statement, or API contract changes are introduced.

## Open questions

None.
