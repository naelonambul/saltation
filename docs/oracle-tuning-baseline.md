# Oracle tuning baseline

Step 1 of `changes/oracle-tuning/plan.md`. Measured numbers stay in the ignored `.evidence/` directory, because the OTN license forbids disclosing benchmark results without Oracle's consent and this repository is public. This report states outcomes, plan shapes and wait-event names only, and names the evidence directory each comes from.

## Host and data volume

- Host: Mac mini, Colima (aarch64), Oracle Database 19c Enterprise Edition (19.19, arm64) in the `saltation-it` Compose project. The Oracle container's CPU and memory limits are recorded in each capture's `host.txt`.
- Data: a separate schema, `SALTATION_TUNE`, seeded by `it-oracle.sh tune-seed` with the defaults (20,000 customers, three simulated years, a hot subset of deep accounts). Every consistency check in `scripts/dev/oracle-tune/check.sql` passed.
- Evidence: seed `.evidence/oracle-tuning/20261116T100519Z-seed/`, capture `.evidence/oracle-tuning/20261116T110139Z/`, both on the Mac mini.

The first full seed ran on the OCI host and filled its shared data disk: the PDB's local undo had no size cap. `tune-seed` now caps PDB undo and checks free space on the Oracle data filesystem itself. Measurement moved to the Mac mini, and every later comparison in this change runs on that host.

## Method

`it-oracle.sh tune-capture` with three runs per entry: `transfer` at concurrency 8, 32 and 64; `transfer_hot` (the 20 deepest seeded accounts) at 32 and 64; `history`, `balance` and `lookup` at 8; and `outbox`, which drives the production relay loop over a large pending backlog.

- Timed windows run with normal statistics. Each window records start and end snapshots of system, session and per-cursor counters (`snap-*`, `sqldelta-*`), and transfer windows also record ASH profiles (`ash-*`).
- Plans come from a separate short run under `STATISTICS_LEVEL=ALL` (`xplan-<entry>.txt`, `DBMS_XPLAN.DISPLAY_CURSOR` with `ALLSTATS LAST`).
- Statements driver-bench does not exercise are executed once by `tune_coverage` with the production binds, inside a transaction that is rolled back, and their plans are read in the same session (`xplan-coverage.txt`).
- The outbox is drained and truncated before every run, and schema statistics are gathered once before the first window.

Three reviewers (Fable, Astra and Hydrogen) read the tooling and both captures independently. The verdicts below are the ones they agreed on.

Limits of this baseline:
- Run-to-run variation on this host is large. Transfer tail latency follows long `log file sync` waits rather than SQL. Later steps are judged on per-request counters (commits, rollbacks, round trips, executions and buffer gets per request) and on throughput, not on p95 alone.
- `DEPOSIT_DETAIL`, `PASSWORD_RESET` and `ATM_SESSION` are empty in the seed, so the coverage plans for those tables show shape, not behaviour at scale.
- The truncate before each outbox run reloads the backlog cursor, so the outbox window has no per-cursor delta for `select_backlog`.

## Summary

The dominant wait in every transfer window is `log file sync`, followed by CPU. Every authenticated request commits twice: once for the token and session touch, once for the request's own work. `enq: TX - row lock contention` is negligible under uniform traffic. It appears on hot accounts, where it is secondary to commit waits and some requests are answered busy.

Several costs the plan did not list are confirmed: the two commits per request, a rollback after every database unit of work even when it already committed, a separate `clock.select` per request, and a full per-account index scan for the latest business date in `select_balance`.

## Finding 1: Lock hold time in posting

Status: refuted as the main limit; confirmed as a secondary cost on hot accounts.

- Uniform transfers: ASH is dominated by `log file sync` and CPU, with almost no row-lock samples, at every concurrency. Throughput is flat from concurrency 8 upward, while latency grows behind the 12-session pool.
- Hot accounts: `enq: TX - row lock contention` appears on `ledger.lock_accounts`, still well behind `log file sync`, and some requests are answered busy.
- Each statement between `lock_accounts` and the commit executes once per transfer (`sqldelta-transfer-*`), so the round-trip count in the plan is confirmed. The commit's log sync is the largest part of the lock hold.
- `outbox.session_device` never executes on the transfer path: refuted, the transfer handler already passes the device.
- `transfers.registered` runs after the lock and does not depend on it: confirmed.

## Finding 2: History pagination

Status: confirmed.

`select_transactions_desc` and `select_transactions_asc` both range-scan `ACCOUNT_TRANSACTION_TIME_IX` over the whole window, fetch each row from the table, and apply `WINDOW SORT PUSHED RANK` before returning the first page (`xplan-history-8.txt`, `xplan-coverage.txt`). The `select_balance` call made by the history handler adds the full scan described under "Further findings". Deep pages and old windows were not timed.

## Finding 3: Outbox relay

Status: confirmed for the backlog count; unclear for leaf density.

- `outbox.select_backlog` is a `TABLE ACCESS FULL OUTBOX`, and the production relay loop runs it before every batch (`xplan-outbox-1.txt`). After a drain the scan still reads up to the table's high-water mark.
- `OUTBOX_CREATED_IX` is used by neither `select_backlog` nor `select_batch`.
- `select_batch` reads emptied `OUTBOX_PK` leaf blocks after a drain. How much this costs after a long backfill was not measured, because the capture truncates the outbox.

## Finding 4: Redundant round trips

Status: confirmed.

- Auth: `select_token`, `extend_token` and `touch_session` each execute once per authenticated request, with their own commit.
- `auth.record_failure` then `auth.select_failures`, and the ATM `lock_session`/`session` and `lock_credential`/`credential` pairs, read the same row twice by unique key (`xplan-coverage.txt`). These are cold paths.
- `daily_close.count_transfers` and `daily_close.transfers` run the same transfer-shape aggregation over the whole day, each with a large group-by.

## Finding 5: Indexes that match no query

Status: confirmed for payees and password resets; unclear for deposit maturity.

- `transfers.registered` uses `PAYEE_CUSTOMER_IX` and filters the recipient and deletion predicates on the table, not `PAYEE_ACTIVE_UK`. Cheap with few payees per customer.
- `password_reset.peek_code` and `select_code` aggregate after table access through `PASSWORD_RESET_CUSTOMER_IX`, with no index-only MIN/MAX path.
- `deposits.candidates` scans the primary key of a nearly empty table, so the maturity index question stays open.

## Finding 6: Write cost of unused indexes

Status: partly confirmed.

- `TRANSACTION_REQUEST_BTID_IX` and `LOGIN_ATTEMPT_PHONE_IX`: no reader under `bank/src`, and no captured plan uses them. The spec requires login attempts to be indexed by phone and time, so `LOGIN_ATTEMPT_PHONE_IX` stays. For `TRANSACTION_REQUEST_BTID_IX`, readers in the generator, admin app and tests are still to be checked.
- `ACCOUNT_TRANSACTION_DATE_IX`: read, by the `select_balance` latest-date subquery only. The spec requires this index, so it stays; the subquery is what needs rewriting.
- `ACCOUNT_DORMANCY_IX`: read by `accounts_lifecycle.candidates` as an `INDEX FAST FULL SCAN`, so it gives no ordering benefit.
- The write cost of each index was not isolated.

## Finding 7: Physical design for backfill row counts

Status: sizes confirmed; benefits unclear; `INITRANS` refuted.

- `segments.txt` confirms the scale: `TRANSACTION_REQUEST` is the largest segment, the account-leading ledger indexes are large, and `DAILY_CLOSE_PK` plus `DAILY_CLOSE_DATE_IX` are larger than their table.
- `daily_close.transfers` reads `DAILY_CLOSE_DATE_IX`, so dropping it needs the replacement key first.
- No capture measured the benefit of compression, an IOT or `PCTFREE`.
- `enq: TX - allocate ITL entry` is negligible in every window, so `INITRANS` on `account` is not needed.
- An IOT for `daily_account_total` keyed by date first depends on finding 8.

## Finding 8: DATE bind types

Status: confirmed for `add_daily`; refuted for `lock_daily`.

- `ledger.add_daily` accesses `DAILY_ACCOUNT_TOTAL_PK` by account only and filters `"S"."BUSINESS_DATE"=INTERNAL_FUNCTION("D"."BUSINESS_DATE")`.
- `ledger.lock_daily` uses an `INDEX UNIQUE SCAN` with a plain date predicate.
- `deposits.candidates` compares the maturity date directly, but its table is too small to show the access path.

## Further findings

Not listed in the plan; confirmed by all three reviewers.

1. **Two commits per authenticated request.** Every authenticated request, transfers and reads alike, commits once for the auth touch and once for its own work and outcome event, and each commit waits on `log file sync`. This is the largest single cost on this workload.
2. **Rollback after every database unit of work.** `Db::run` rolls back after its closure even when the closure committed or only read. Transfers issue several rollbacks per request and reads more, all transaction-control round trips.
3. **`clock.select` per request**, in its own pool acquisition with its own rollback.
4. **`select_balance` latest business date.** The scalar `MAX(business_date)` is unnested into a group-by over every `ACCOUNT_TRANSACTION_DATE_IX` entry of the account, so its cost grows with account depth. It is the costliest statement of the balance workload, and history pays it too.
5. **Outbox high-water mark.** The backlog scan's cost follows the largest backlog the table has held, not the current one.
6. **Pool size.** On this host, twelve sessions set the latency at concurrency 32 and 64; throughput does not rise above concurrency 8. Whether a different pool size changes throughput was not tested. On hot accounts, `ORA-30006` makes the pool replace sessions.
7. **Statistics on timestamp-with-time-zone indexes.** Their function-based columns are badly misestimated, which is the plan's statistics-dependence risk.
