# Implementation Plan

Status: draft for the owner. Not approved.

## Summary

Tune the bank's Oracle access paths, lock hold time and indexes, without changing any API, spec rule or stored meaning.

The work starts from a read-only review of every migration and every production SQL file by three reviewers (Fable, Astra and Hydrogen, 2026-11-15). Each one ranked lock hold time in the posting path as the main limit on throughput. Measured on the OCI host, transfer throughput levels off from concurrency 32 upward, and p95 grows with concurrency. The slowest statement is `ledger.lock_accounts`. Its plan is a primary-key IN-list, so the time is spent queueing behind transactions that hold the account rows.

Every finding is a hypothesis until a plan or wait profile confirms it. Step 1 measures, and each later step lands only when its proof shows a gain, or at least no loss.

**Dependency.** This change is based on `main` but needs `oracledb-driver` merged first. It uses that change's adapter and session pool, and the `driver-bench` target as the before/after harness. Rebase this branch onto `main` after `oracledb-driver` merges and before step 1.

**Invariants kept** (spec F-22, F-24, F-36 and the auth rules):
- accounts are locked with `SELECT ... FOR UPDATE` in ascending account-ID order;
- each operation is one database transaction;
- transaction rows are never updated or deleted;
- the outbox delivers in event-ID order, in batches of 500, and deletes by delivered ID;
- the token's 30-minute sliding expiry is unchanged.

## Findings to confirm, by expected impact

1. **Lock hold time in posting** (all three reviewers). About 9 to 10 round trips sit between `lock_accounts` and the commit in `shared/ledger/mod.rs::post` and `features/transfers/mod.rs`: `lock_daily`, `registered`, `insert_transfer`, `insert_row`, `update_account`, `add_daily`, `finish_request`, `session_device`, `insert_event`, commit. Candidates:
   - move `transfers.registered` before the lock (it does not depend on it);
   - stop issuing `outbox.session_device` when the caller already knows the device;
   - post the transfer, ledger rows, balance updates, daily-total MERGE, `finish_request` and the outbox insert in one static, bound PL/SQL block;
   - read the daily totals in the locking statement (Hydrogen: join `daily_account_total` with `FOR UPDATE OF a.balance`).
2. **History pagination** (all three). The hint in `accounts/sql/select_transactions_{asc,desc}.sql` forces `account_transaction_time_ix`, so every row in the window is fetched and sorted before the first page is returned. Drive the query by `(account_id, account_sequence)` in the cursor's direction so it stops at the page size. Compare first pages, deep pages, recent windows and old windows. `select_balance`, which this handler calls only for three columns, gets a narrower lookup.
3. **Outbox relay** (all three). `select_backlog` counts the whole backlog before every 500-row batch. Sample the backlog metrics on a timer instead. Decide whether to keep `outbox_created_ix` from the measurement (the reviewers disagree). Check `outbox_pk` leaf density after a long backfill before adding any `COALESCE`.
4. **Redundant round trips** (all three):
   - auth: `select_token` + `extend_token` + `touch_session`;
   - `auth/record_failure` then `select_failures` (use `RETURNING`);
   - ATM `lock_session` then `session`, and `lock_credential` then `credential`;
   - `daily_close` runs the same transfer-shape aggregation in `count_transfers` and in `transfers`.
5. **Indexes that match no query** (two or three reviewers):
   - `payee_active_uk` is not used by `transfers/sql/registered.sql` (rewrite the predicates as the index's CASE expressions);
   - `password_reset_customer_ix` should be `(customer_id, reset_id)` for the `MAX(reset_id)` lookups;
   - `deposit_detail` maturity index: index only open deposits (`CASE WHEN closed_at IS NULL THEN maturity_date END`).
6. **Write cost of unused indexes**: `transaction_request_btid_ix` (no query), `account_dormancy_ix` (rewritten on account activity for a once-a-day scan), `account_transaction_date_ix` (only `MAX(business_date)` in `select_balance`), `login_attempt_phone_ix`. Drop each only after confirming no reader under `bank/src`, the generator, the admin app or the tests uses it.
7. **Physical design for the backfill's row counts**:
   - `daily_close` primary key as `(business_date, account_id) COMPRESS 1`, dropping `daily_close_date_ix` (optionally an IOT);
   - `daily_account_total` as an IOT keyed `(business_date, account_id)` (only with a confirmed DATE access predicate);
   - `COMPRESS 1` on the account-leading ledger indexes;
   - `PCTFREE` for `transaction_request` (its `response_json` is filled by a later update);
   - `INITRANS` on `account` only if `enq: TX - allocate ITL entry` appears.
8. **Bind type of DATE columns.** Two reviewers flagged `NaiveDate` binding as TIMESTAMP against DATE columns. The `oracledb-driver` plan check already showed `lock_daily` with an `INDEX UNIQUE SCAN` and a plain `"BUSINESS_DATE"=:BUSINESS_DATE` predicate. Confirm the same for `add_daily` and `deposits/candidates`, and fix only a statement whose plan shows `INTERNAL_FUNCTION`.

**Not planned:**
- reverse-key or hash-partitioned primary keys: with 12 sessions on one instance, two reviewers judged them not applicable, and they would break the range scans the outbox and sequence indexes need;
- partitioning: a separately licensed Enterprise Edition option, and of little value while nothing is purged;
- speculative analyst-search indexes: no analyst SQL exists yet.

## Files and components that change

- `bank/migrations/0017_*.sql` and later: index, key and storage changes. Migrations are forward-only; never edit an applied migration.
- `bank/src/shared/ledger/**`, `bank/src/features/transfers/**`: the posting path and its SQL.
- `bank/src/features/accounts/**`: history and balance SQL.
- `bank/src/shared/outbox/**`, `bank/src/features/outbox_relay/**`: backlog sampling.
- `bank/src/shared/auth/**`, `bank/src/features/auth/**`, `bank/src/features/atm/**`, `bank/src/features/password_reset/**`, `bank/src/features/payees/**`, `bank/src/features/deposits/**`, `bank/src/features/daily_close/**`: round trips and index-matching predicates.
- `bank/tests/**`: plan-shape tests and any new integration tests.
- `scripts/dev/**`: a measurement target (step 1).
- `docs/**`: the measurement report.

## Order of work

1. **Measure.**
   - Add an `it-oracle.sh` target that seeds a multi-year synthetic population, at least 20,000 customers with ledger rows in the tens of millions.
   - Capture `DBMS_XPLAN.DISPLAY_CURSOR` (ALLSTATS LAST) for every statement in the findings, and ASH/AWR wait profiles during `driver-bench` transfer windows at concurrency 8, 32 and 64.
   - Write the result to `docs/oracle-tuning-baseline.md`: each finding confirmed, refuted or unclear, with its evidence.
   - Refuted findings are dropped. This plan is revised and re-approved if the confirmed set differs materially from the findings above.
   - Proof: the report exists, and the `driver-bench` JSON for this commit is kept under `.evidence/`.
2. **Posting path** (finding 1). Proof: `it-oracle.sh all` and `outbox-outage` pass, and `driver-bench` transfer windows at 32 and 64 show higher TPS or lower p95 than step 1 across three runs.
3. **History pagination** (finding 2). Proof: a plan-shape test asserts no `SORT ORDER BY` on the first page, the existing history tests pass, and the `driver-bench` history window is not slower.
4. **Outbox relay** (finding 3). Proof: `outbox-outage` and the `driver-bench` outbox window pass, and the backlog metrics are still exported.
5. **Round trips** (finding 4). Proof: `it-oracle.sh all`, including the ATM, auth and daily-close suites.
6. **Index and physical changes** (findings 5 to 8), one migration per group. Proof: `it-oracle.sh migrations` on a fresh and on a populated schema, plan-shape tests for the rewritten predicates, and `driver-bench`.
7. **Full verification.** `python3 scripts/repo.py verify --change oracle-tuning --full` on the OCI host, then `migrations` and `e2e`, plus a final `driver-bench` compared with step 1.

## Tests and proof

- No assertion is relaxed. The 200-concurrent-duplicate test, the random-transfer conservation test, rollback cases, `outbox-outage`, `observability` and `e2e` stay unchanged and passing.
- New plan-shape tests use `EXPLAIN PLAN` or `DBMS_XPLAN` to assert the intended access path for each rewritten statement, so a later edit cannot silently lose it.
- Each step that claims a speed-up keeps its `driver-bench` JSON next to step 1's, same host and same container limits.
- Measured numbers (throughput, latency, wait times, plan statistics such as A-Time or buffers) stay in the ignored `.evidence/` directory. `docs/oracle-tuning-baseline.md`, commit messages and pull request text state each outcome (confirmed, refuted, faster, not slower), plan shapes and wait-event names, and point to the evidence path, because the OTN license forbids disclosing benchmark results without Oracle's consent and this repository is public (owner decision, 2026-11-16).

## Risks and mitigations

- **A PL/SQL posting block hides which statement failed.** Keep the existing error mapping (ORA-30006 and outbox-full codes to busy) and log the failing step from the block.
- **Index changes on a populated schema take locks and time.** Use `ONLINE` builds where Enterprise Edition allows, and run `migrations` against a populated schema in step 6.
- **A gain on the benchmark is not a gain on the mixed backfill.** Step 7 also runs the generator backfill for a fixed simulated period and compares its sustained rate with N-4 (200 operations/s).
- **Optimizer statistics go stale during a long backfill and plans flip.** Step 1 records whether the plans depend on statistics. If they do, the plan adds a statistics step at rollover.

## Rollback or recovery

Code changes revert with the change's squash commit. A migration that drops or rebuilds an index is undone by a new forward migration that recreates it; record the original DDL in each migration's header comment so the restore is mechanical.

## Open questions

- Commit mode for the auth touch: `COMMIT WRITE BATCH NOWAIT` would save a log-sync wait per request, but a crash could lose the last token extension. Owner decision.
- Skipping the token-extension update when the expiry moved less than about a minute saves more, but it changes the 30-minute sliding rule in the spec. Owner decision; it would need a `behavior` change.
- `outbox_created_ix`: keep or drop, decided by the step-1 measurement.
- Data volume for step 1: how many simulated years, within the 70 GB budget on the OCI host.
