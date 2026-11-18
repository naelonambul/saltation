# Implementation Plan

Status: draft for the owner. Not approved.

## Summary

Tune the bank's Oracle access paths, commit and round-trip overhead, and indexes, without changing any API, spec rule or stored meaning.

The work started from a read-only review of every migration and production SQL file by three reviewers (Fable, Astra and Hydrogen, 2026-11-15), who ranked lock hold time in the posting path first. Step 1 measured a populated schema (20,000 customers, three simulated years) and refuted that ranking. The dominant wait is `log file sync`, and every authenticated request commits twice: once for the token touch, once for the request's work. Row-lock contention is negligible under uniform traffic and secondary on hot accounts. The verdicts, plan shapes and wait events are in `docs/oracle-tuning-baseline.md`; the numbers stay in `.evidence/oracle-tuning/` (see "Tests and proof").

This revision reorders the work by measured cost: per-request overhead first, then the history and balance reads, then the posting path, the outbox, the cold paths and the indexes.

**Measurement host.** Step 1 and every later comparison run on the Mac mini (Colima, Oracle 19c EE arm64) with `it-oracle.sh tune-seed` and `tune-capture`. The OCI host keeps running CI. A comparison is valid only on the same host, the same container limits and a fresh seed.

**Invariants kept** (spec F-2, F-3, F-22, F-24, F-25, F-26, F-35, F-36):
- accounts are locked with `SELECT ... FOR UPDATE` in ascending account-ID order;
- each operation is one database transaction;
- transaction rows are never updated or deleted;
- the outbox delivers in event-ID order, in batches of 500, and deletes by delivered ID;
- the token's 30-minute sliding expiry is unchanged, and a revoked token or ended session is never revived;
- every login attempt is stored, and the failure lockout rule is unchanged;
- every request's outcome is recorded once;
- the indexes named in the spec's data section stay, including transaction rows by (account, business date, account sequence) and login attempts by phone and time.

## Findings, after step 1

Verdicts from `docs/oracle-tuning-baseline.md`. Refuted items are dropped.

1. **Two commits per authenticated request** (new, confirmed). The `AuthCustomer` extractor (`shared/auth/mod.rs`) runs `select_token`, `extend_token` and `touch_session` and commits on its own pool acquisition. The request's completion then commits separately, either in the handler (`Session::note_completion` before its commit) or in `outcome::Recorder::finish`. This holds for reads too.
2. **Transaction-control round trips** (new, confirmed):
   - `Db::run` rolls back after every unit of work, even after a commit or a read-only closure;
   - the pool marks a session unhealthy on any error, so `ORA-30006` on hot accounts replaces sessions.
3. **History and balance reads** (finding 2, confirmed; new `select_balance` item):
   - `select_transactions_{asc,desc}` range-scan `account_transaction_time_ix` over the whole window, then `WINDOW SORT PUSHED RANK` before the first page;
   - `select_balance`'s `MAX(business_date)` is unnested into a group-by over every `account_transaction_date_ix` entry of the account, so its cost grows with account depth. History pays it too.
4. **Posting path** (finding 1, revised; finding 8):
   - `add_daily` binds `business_date` as TIMESTAMP, and its plan filters `INTERNAL_FUNCTION("D"."BUSINESS_DATE")` after an account-only range scan;
   - `transfers.registered` runs after `lock_accounts` and does not depend on it;
   - row-lock contention on hot accounts is secondary to commit waits.
5. **Outbox relay** (finding 3, confirmed). `select_backlog` scans the table to its high-water mark before every relay batch. `outbox_created_ix` is used by no query.
6. **Cold-path round trips** (finding 4):
   - `daily_close.count_transfers` and `daily_close.transfers` repeat the same aggregation;
   - `auth/record_failure` then `auth/select_failures`;
   - the ATM `lock_session`/`session` and `lock_credential`/`credential` pairs.
7. **Indexes and physical design** (findings 5 to 7):
   - `transfers/registered.sql` predicates do not match `payee_active_uk`;
   - `password_reset_customer_ix` gives no index-only path for the `MAX(reset_id)` lookups;
   - `transaction_request_btid_ix` has no reader under `bank/src`;
   - `daily_close` has a primary key and `daily_close_date_ix` larger than the table;
   - the account-leading ledger indexes and `transaction_request` are the largest segments.

**Dropped by step 1:**
- skipping `outbox.session_device` on transfers: it never executes there;
- `INITRANS` on `account`: `enq: TX - allocate ITL entry` waits are negligible;
- the `lock_daily` bind type: its plan is already a unique scan.

**Dropped because the spec requires them:** dropping `account_transaction_date_ix` or `login_attempt_phone_ix` (spec data section). Step 5 makes `select_balance` use `account_transaction_date_ix` efficiently instead.

**Kept open for a populated fixture (step 2 seeds them):**
- the open-deposit maturity index;
- `OUTBOX_PK` leaf density after a long backlog;
- an IOT for `daily_account_total` (only after step 6 fixes the `add_daily` bind).

**Not planned:**
- reverse-key or hash-partitioned primary keys: with 12 sessions on one instance they do not apply, and they would break the range scans the outbox and sequence indexes need;
- partitioning: a separately licensed Enterprise Edition option, of little value while nothing is purged;
- speculative analyst-search indexes: no analyst SQL exists yet;
- pool size changes: twelve sessions set latency at high concurrency, not throughput, on this host;
- caching the business clock in memory: `clock.select` keeps running per request inside the `operation` middleware (F-25, F-26); step 3 only removes its trailing rollback;
- a single PL/SQL posting block, and reading daily totals in the locking statement: both are deferred unless step 6's capture still shows row-lock contention as material, which would need a plan revision;
- `ACCOUNT_DORMANCY_IX`: read once a day by `accounts_lifecycle.candidates`, small, kept.

## Files and components that change

- `bank/migrations/0017_*.sql` and later: index, key and storage changes. Migrations are forward-only; never edit an applied migration.
- `bank/src/shared/db/mod.rs`: transaction tracking, rollback only when needed, session health on `ORA-30006`, and running a pending token touch before the completion commit.
- `bank/src/shared/auth/mod.rs` and `bank/src/shared/auth/sql/**`: read-only token validation and the guarded touch statements.
- `bank/src/shared/outcome/mod.rs`: the completion transaction applies the pending touch.
- `bank/src/shared/ledger/**`, `bank/src/features/transfers/**`: the posting path and its SQL.
- `bank/src/features/accounts/**`: history and balance SQL.
- `bank/src/shared/outbox/**`, `bank/src/features/outbox_relay/**`: backlog sampling.
- `bank/src/features/auth/**`, `bank/src/features/atm/**`, `bank/src/features/password_reset/**`, `bank/src/features/payees/**`, `bank/src/features/daily_close/**`: round trips and index-matching predicates.
- `bank/tests/**`: plan-shape tests, transaction and concurrency tests, the measurement harness.
- `bank/Dockerfile`: invalidate the bank package before a cache-mounted runtime build; retain dependency caches.
- `scripts/dev/**`: the measurement targets, counter report and source-aware Rust cache runner.
- `docs/**`: the measurement report.

## How results are judged

- **Authenticated entries** are `transfer`, `transfer_hot`, `history`, `balance` and `lookup`. `outbox` is unauthenticated and is judged only in step 7.
- **Per-request counters** come from each timed window's start and end snapshots (`snap-*`, `sqldelta-*`), divided by the window's successful requests: commits, rollbacks, `redo synch writes`, SQL*Net round trips, and executions and buffer gets per statement. Step 2 adds a script that writes them as one table per capture (`counters.txt`).
- **Throughput** is judged on the median of three runs. "Higher" means the median is above step 2's highest run. "Not lower" means the median is at or above step 2's lowest run.
- p95 alone is not a criterion: on the Mac mini the transfer tail follows long `log file sync` waits.

## Order of work

1. **Measure.** Done: `docs/oracle-tuning-baseline.md`, evidence `.evidence/oracle-tuning/20261116T100519Z-seed/` and `20261116T110139Z/` on the Mac mini.
2. **Harness follow-ups** (`scripts/dev/oracle-tune/**`, `scripts/dev/it-oracle.sh`, `bank/tests/driver_bench.rs`, `bank/tests/oracle_tune_coverage.rs`).
   - Record the Oracle container's limits in every result line.
   - Add `scripts/dev/oracle-tune/counters.py`, which writes `counters.txt` as defined above.
   - Take the outbox backlog delta without reloading its cursor: truncate before the warm-up, not between warm-up and window.
   - Seed time deposits for 10% of customers with maturities spread over the seeded years, password resets for 2% of customers, ATM sessions for 1% of customers, and three payees for each of 1,000 customers.
   - Add `history` cases for a first page and a deep page, each on a recent and an old window, on shallow and deep accounts, plus one filtered case (deposits only) on a deep account.
   - Add an `outbox_long` measurement: queue a backlog of 500,000 events with the relay stopped, drain it without truncating, and capture `select_batch` and `select_backlog` plans and gets after the drain. This decides `OUTBOX_PK` leaf density.
   - Make shared Rust build caches source-aware before continuing staged captures. The first step-4 capture reused the preceding candidate's executable despite changed source bytes; preserve it under `.evidence/handoff/step4/` as invalid evidence. The isolated Cargo reproduction and artifact chronology are recorded there too.
     - Add a runner under `scripts/dev/` that fingerprints the bank's compiler inputs, embedded migrations/test fixtures, Cargo configuration/lockfile, relevant contracts and compiler identity. Use one cache-local lock across input inspection, bank-package invalidation and the Cargo command. Key trust markers by the effective profile and target, and match invalidation to that profile: debug/test/clippy use the debug artifacts, release requires `cargo clean --release --package bank --locked`, and a supported custom profile uses its matching `--profile`. Never let a successful debug command declare older release artifacts fresh. Missing, untrusted or changed fingerprints force bank-package cleaning; retain registry and dependency artifacts. Before any cleaning or Cargo execution, invalidate the previous successful marker for that profile/target and mark it untrusted. Restore trust atomically only after a successful command with unchanged inputs, and retain provenance. A failed or interrupted command must leave that state untrusted even when returning to a previously successful source fingerprint.
     - Put guarded builds in a dedicated target-cache volume and a named BuildKit target-cache ID, separate from older branches/jobs that do not honor the new lock. Leave existing target caches untouched and reuse the registry cache. Budget disk space for the initial dependency build in the new namespaces; preserve those dependency artifacts on later bank-only invalidations. Insufficient space is a reported blocker, not permission to purge unrelated caches.
     - Route compiling commands in `ci-rust.sh` and the integration/benchmark runners through it. Mount the same read-only contracts inputs for both paths. Formatting and lockfile-generation commands retain their established behavior. Preserve tool exit statuses and output ownership; host timeout/cancellation must stop the invocation's container before returning, without leaving an orphaned writer or claiming a completed build.
     - In `bank/Dockerfile`, serialize the named target-cache mount with `sharing=locked` and run `cargo clean --release --package bank --locked` before the existing cache-mounted release build. Docker may reuse an already completed layer for identical inputs; whenever the build command executes, it must compile the current package rather than trust copied file timestamps. Keep the runtime image, dependency caches and security settings unchanged.
     - Proof: a real isolated Cargo fixture switches A to B and back to A with old, equal file timestamps and produces the current executable each time, in debug and release, including interleaved debug/release source switches. An unchanged repeat reuses the bank artifacts; dependency artifacts survive bank-only invalidation. Also exercise A-success, B-compiles-then-test-failure-or-cancellation, A-return: the final A must clean and rebuild despite its previously successful fingerprint. Failed/interrupted commands cannot retain or advance trusted provenance, and concurrent guarded callers cannot mix source states. A concurrent legacy writer uses the old namespace and cannot alter guarded artifacts. Exercise the Docker build path with changed source and preserved old timestamps as well.
     - Audit the existing step-2/3 build logs, source chronology and observed SQL identities before retaining their comparisons. If their source/binary alignment cannot be established, repeat the affected reference/candidate from its original source and a fresh identical seed; preserve the original evidence and acceptance thresholds.
     - Before another full step-4 capture, run a short populated diagnostic that proves the deferred-touch SQL executes, retired immediate-touch SQL does not, and the expected commit change is present. Then take a fresh default seed and repeat the full comparison. Apply the same source-provenance gate to later stages and canonical verification; do not adopt the aborted stale-binary capture.
   - Proof: `tune-seed` then `BENCH_RUNS=3 tune-capture` on the Mac mini writes `counters.txt`. The finding 1 and 2 verdicts (dominant wait, row-lock share on hot accounts, the history plan shape) are unchanged from step 1. This capture is the reference for steps 3 to 9.
3. **Session overhead** (finding 2; `shared/db/mod.rs`).
   - The session records that a transaction may be open before it runs any statement other than a plain `SELECT`, including a statement that then fails. The flag is set by method: always for `execute`, `execute_batch`, `insert_returning_id` and PL/SQL, and for `query_*` when the statement text contains `FOR UPDATE` (case-insensitive; a false positive only costs a rollback). It clears only when a commit or rollback succeeds.
   - After the closure: roll back if it returned an error or the flag is set; otherwise return the session to the pool without a rollback. The flag clears only after a commit or rollback succeeds.
   - After any error, roll back. The session stays in the pool only if that rollback succeeds and the error's kind is not a call timeout (`kind()` is `CallTimeoutExceeded`); otherwise it is retired, as on a panic or a failed commit today. This uses only the driver's public `kind()` and `db_error_code()`, read before the error is mapped to `DbError::Busy`.
   - Proof:
     - `it-oracle.sh all` passes, including the pool-health and rollback tests;
     - a new test: a closure locks a row with `FOR UPDATE` and returns `Ok` without committing, and another connection can then lock that row;
     - a new test, on a pool of one session: after `ORA-30006`, that session serves the next unit of work with no open transaction (same SID);
     - rollbacks per successful request are zero on `history`, `balance` and `lookup` and lower on `transfer`. Round trips per request are lower on every authenticated entry, and `logons cumulative` does not grow during `transfer_hot`. Throughput is not lower.
4. **One commit per authenticated request** (finding 1; `shared/auth/mod.rs`, `shared/auth/sql/**`, `shared/db/mod.rs`, `shared/outcome/mod.rs`).
   - `AuthCustomer` validates with one `select_token` read. It writes nothing and stores a pending touch (token hash, session ID, business time) in the request's `RequestStats` metadata, which `Db::run_in` already passes to the session. Every authenticated request defers its touch; there is no immediate-commit branch.
   - When a session commits a request's completion (`Session::commit` while `note_completion` is pending, or `Recorder::finish` when no completion was committed), it first runs the pending touch in that transaction. The pending touch clears only after that commit succeeds, together with `completion_committed`, so a rolled-back or failed handler commit leaves it for `finish`, and `finish` never applies it a second time. The touch is one bound PL/SQL block. It updates the token with `expires_at = GREATEST(expires_at, :now + 30 minutes)` `WHERE revoked_at IS NULL`, then the session with `last_seen = GREATEST(last_seen, :now)` `WHERE ended_at IS NULL`, in that order (token before session, as logout and password reset do).
   - The touch is the last statement before the commit, so the token and session rows are held only for the commit. The block locks the token row, then the session row, by primary key and without the revoked or ended guards, with `SELECT ... FOR UPDATE WAIT 1`, so logout, which revokes both before its commit, still commits with the touch as a no-op; and a lock timeout (`ORA-30006`) maps to busy, like `lock_accounts`.
   - Every request after a successful `AuthCustomer` has exactly one completion commit, whether the handler commits it, rolls back first (idempotent replay, errors) and leaves it to `finish`, or rejects with 400 or 403. So every authenticated request commits the touch exactly once. "Pending" means the request's completion is not yet committed (`completion_committed` is false on the request's shared `RequestStats`). If the handler's completion commit fails or its touch times out, the touch stays pending and `finish` commits it with the error response's completion. Only if `finish` also fails is the touch lost, and that response is already busy. This is accepted: the token stays valid until its previous expiry.
   - Proof:
     - `it-oracle.sh all` passes, including the auth, expiry, lockout, logout, password-reset, rejected-request and idempotent-replay tests;
     - new tests: a request rejected with 403, a replayed transfer and a 400 after authentication each extend the token; a slower request finishing after a newer one does not move `expires_at` or `last_seen` backwards; a token revoked by logout or password reset while a request is in flight stays revoked; two concurrent requests with the same token both succeed;
     - commits per request fall by one on every authenticated entry, with `redo synch writes` falling with them;
     - throughput is higher on `transfer` at concurrency 8 and on `lookup`;
     - on `transfer_hot`, busy responses are not higher than step 2's highest run;
     - the 200-concurrent-duplicate test, whose requests all share one token, passes unchanged with no more busy responses than before; the touch's wait bound is chosen against it, because a touch timeout rolls back finished work in the handler and turns a success into busy in `finish`.
5. **History and balance reads** (finding 3; `features/accounts/**`).
   - Drive history pagination by `(account_id, account_sequence)` in the cursor's direction, so the query stops at the page size. Old and filtered windows may scan rows outside the window; their cost is bounded by the proof below.
   - Rewrite `select_balance`'s latest business date to read one `account_transaction_date_ix` entry, through an index-only descending or MIN/MAX access. Give the history handler a balance lookup without the columns it discards.
   - Proof: plan-shape tests assert no `WINDOW SORT` or `SORT ORDER BY` before the first page of history, and an index-only one-entry access for the latest date. The history and balance tests pass. Buffer gets per execution of `select_balance` are the same on shallow and deep accounts. In each step 2 history case, including the old-window and filtered ones, gets per execution are not higher than step 2's, and lower for recent first pages. Throughput on `history` and `balance` is higher.
6. **Posting path** (finding 4; `shared/ledger/**`, `features/transfers/**`).
   - Bind `add_daily`'s `business_date` as DATE.
   - Move `transfers.registered` before `lock_accounts`.
   - Proof: a plan-shape test shows `add_daily` with an index access on both key columns and no `INTERNAL_FUNCTION`. The conservation, duplicate, rollback, daily-limit and `outbox-outage` tests pass. Statements executed between `lock_accounts` and the commit fall by one per transfer, and throughput on `transfer` is not lower.
7. **Outbox relay** (finding 5; `shared/outbox/**`, `features/outbox_relay/**`).
   - Remove the backlog query from the relay batch loop, and refresh the backlog metrics on a timer of a few seconds.
   - Rewrite the oldest-age part to read `outbox_created_ix` (index-only MIN/MAX), so the index gains a reader and stays.
   - Proof: `outbox-outage` and `observability` pass. The backlog metrics are still exported and change while a backlog drains. `select_backlog` executions in the `outbox` window are at most one per timer interval, no longer one per batch, and that window's throughput is higher.
8. **Cold paths** (finding 6).
   - `daily_close`: compute the transfer-shape aggregation once and derive the count from the same rows (`daily_close/sql/transfers.sql`, `daily_close/sql/count_transfers.sql`, `features/daily_close/mod.rs`).
   - `auth`: `record_failure` returns the failure count with `RETURNING`, and `select_failures` goes.
   - ATM: `lock_session` and `lock_credential` select the columns the follow-up reads return, and `session` and `credential` go from those paths.
   - Proof: `it-oracle.sh all` passes (its ATM, auth and daily-close tests). The removed SQL files are gone. `tune_coverage` shows one aggregation in the daily-close plans.
9. **Indexes and physical changes** (finding 7), one migration per group, each with the original DDL in its header comment.
   - Group A (predicates): rewrite `transfers/registered.sql` to match `payee_active_uk`, and recreate `password_reset_customer_ix` as `(customer_id, reset_id)`.
   - Group B (unused index): drop `transaction_request_btid_ix`, only after a search of `bank/src`, `generator/`, the admin app and `bank/tests` finds no reader.
   - Group C (`daily_close`): make the primary key `(business_date, account_id)` with `COMPRESS 1`, and drop `daily_close_date_ix` once `daily_close/sql/transfers.sql` reads the new key.
   - Group D (storage, conditional): `COMPRESS 1` on the account-leading ledger indexes, and `PCTFREE 20` on `transaction_request`. Each lands only if a populated-schema migration shows a smaller segment and `insert_row` gets per execution are not higher.
   - Proof: `it-oracle.sh migrations` on a fresh and on a populated schema; plan-shape tests for groups A and C; `bank/tests/migrations.rs` updated only where a group adds or replaces an index, never to remove a check. Throughput on every entry is not lower than step 2's.
10. **Full verification.** `python3 scripts/repo.py verify --change oracle-tuning --full` on the OCI host, then `migrations` and `e2e` there. A final `tune-capture` on the Mac mini is compared with step 2. The separate fixed-period generator API backfill throughput proof is deferred by the owner's explicit instruction on 2026-11-17. It is not passed and this change does not establish N-4. The three-year SQL seed, consistency checks and every other comparison remain required.

## Tests and proof

- No assertion is relaxed. The 200-concurrent-duplicate test, the random-transfer conservation test, rollback cases, `outbox-outage`, `observability` and `e2e` stay unchanged and passing.
- New plan-shape tests use `EXPLAIN PLAN` or `DBMS_XPLAN` to assert the intended access path for each rewritten statement, so a later edit cannot silently lose it. Statements bound with dates are checked through the real adapter, because `EXPLAIN PLAN` would bind them as strings.
- Each step keeps its `tune-capture` evidence next to step 2's, from a fresh seed, on the same host and container limits.
- Measured numbers (throughput, latency, wait times, counter values, plan statistics such as A-Time or buffers) stay in the ignored `.evidence/` directory. `docs/oracle-tuning-baseline.md`, commit messages and pull request text state each outcome (confirmed, refuted, faster, not slower), plan shapes and wait-event names, and point to the evidence path, because the OTN license forbids disclosing benchmark results without Oracle's consent and this repository is public (owner decision, 2026-11-16).

## Risks and mitigations

- **The deferred touch changes when the extension becomes durable**: at the request's completion instead of before its work. A concurrent request with the same token that arrives after the previous expiry, while an earlier request is still running, would be rejected; today the earlier request's extension is already committed. This needs a request still running at the moment the 30-minute idle window ends. Commercial banks treat a session as extended by each request and a request as lasting seconds, so the owner accepted this difference and chose to defer every touch, with no near-expiry branch or test (2026-11-16: "시중 은행들 처럼 테스트 조건을 완화 하자", choosing "항상 미루기").
- **Lock order across the touch.** A transfer now holds account rows, then takes the token and session rows just before its commit. Logout and password reset take the token, then the session, and hold no account rows, and no path takes account rows after the token. Step 4 audits every statement that locks `access_token` or `auth_session`, and adds the same-token concurrency test. The touch's wait is bounded and maps to busy.
- **Skipping the rollback could leave a transaction or row lock on a pooled session.** The flag is set before any non-plain-`SELECT` statement runs, including failed ones, and step 3's lock-then-`Ok` test proves the lock is released.
- **Keeping sessions after `ORA-30006`** relies on the driver telling connection errors from statement errors. The existing pool-health tests stay unchanged and passing.
- **Index changes on a populated schema take locks and time.** Use `ONLINE` builds where Enterprise Edition allows, and run `migrations` against a populated schema in step 9.
- **A gain on the benchmark is not a gain on the mixed backfill.** The owner deferred the separate generator throughput proof; report this gap without claiming N-4 compliance.
- **Shared Cargo and BuildKit target caches can reuse an older package after a worktree switch.** Source fingerprints, package-only invalidation and the old-timestamp regression witness establish build freshness before measuring or accepting functional results. Preserve invalidated evidence and repeat affected runs.
- **Optimizer statistics go stale during a long backfill and plans flip.** Step 1 found that the outbox plans depend on statistics and that the timestamp-with-time-zone indexes are misestimated. Each rewritten statement gets a plan-shape test, and step 9 decides whether a statistics step at rollover is needed.

## Rollback or recovery

Code changes revert with the change's squash commit. A migration that drops or rebuilds an index is undone by a new forward migration that recreates it; record the original DDL in each migration's header comment so the restore is mechanical.

## Open questions

- If step 4's capture shows the touch's lock queueing on `transfer_hot` as material (busy responses above step 2's highest run), step 4 stops. The alternatives are owner decisions:
  - `COMMIT WRITE BATCH NOWAIT` for a separate touch: it saves the log-sync wait, but a crash could lose the last token extension;
  - skipping the extension when the expiry moved less than about a minute: it changes the 30-minute sliding rule in the spec, so it would need a `behavior` change.
- The open-deposit maturity index, `OUTBOX_PK` leaf density and an IOT for `daily_account_total` are decided from step 2's populated capture and step 6's bind fix. Any of them that is adopted needs a plan revision.
