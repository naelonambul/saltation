# Implementation Plan

## Summary

Replace the `oracle` crate (0.6.3, ODPI-C over Oracle Instant Client) with Oracle's pure-Rust driver `oracledb`, pinned to `=26.0.0-beta.6`, and remove Instant Client from the bank image. The owner decided on 2026-11-15 to make this change (issue #10). No SQL, schema, business rule or API contract changes. Every driver call stays inside `bank/src/shared/db`. The adapter keeps its method names and call shapes (`Db::run`, `Session::execute/insert_returning_id/query_row/query_opt/query_all/execute_batch/commit/rollback`, `DbError::ora_code`), but the types in their signatures change from `oracle`'s to the adapter's own: the bind trait (`crate::shared::db::ToSql`), the row trait used by the query methods, and the error held inside `DbError`. Feature code changes only where it names `oracle::` directly (two imports).

Before the switch, the change adds driver parity tests and a benchmark, run first on the old driver to record a baseline, then again on the new driver on the same host against the same Oracle 19.19 EE container.

Facts checked against the `oracledb` 26.0.0-beta.6 source (crates.io, 2026-11-15):
- The API is synchronous (`Pool::acquire`, `Connection::execute/query/query_row/execute_batch`), so work stays on blocking threads behind a bounded wait (`spawn_blocking`).
- `String`/`&str` bind as VARCHAR (`DB_TYPE_VARCHAR`). The NVARCHAR2 → VARCHAR2 `Bind` wrapper (commit 7903105) is removed only when an execution plan proves it unnecessary.
- `Pool::acquire` has no timeout and blocks until a connection is free or opened; the TCP transport clears its socket timeouts during connect, so opening a connection to a silent listener can block indefinitely.
- `OracleTimestamp` carries no offset, only a UTC flag. Binding a non-UTC value binds `TIMESTAMP`, a UTC value binds `TIMESTAMP WITH TIME ZONE` at UTC. Reading a `TIMESTAMP WITH TIME ZONE` column returns the instant normalised to UTC; the stored offset is not returned. There are chrono conversions on `OracleTimestamp` but no `ToDbValue`/`FromDbValue` for chrono types.
- At connect, the driver sends `ALTER SESSION SET TIME_ZONE` from the `ORA_SDTZ` environment variable, or from the process's local offset when it is unset. This applies to every connection, pooled or not.
- There is no tuple row conversion; rows are read by column (`Row::get`).
- `bool` binds as SQL BOOLEAN, which Oracle 19c does not have.
- Errors expose `db_error_code()` and `ErrorKind::NoDataFound`. DML RETURNING (`ExecResult::returned_data`) and array DML (`execute_batch`) exist.
- When a pooled connection is returned with a transaction open, the driver rolls back with no timeout while holding the pool's mutex.
- Accepted password verifiers: 11G and 12C. No native network encryption (the test database uses neither NNE nor 10G verifiers).
- License: UPL-1.0 OR Apache-2.0. MSRV 1.89 (the host toolchain is 1.99.1).

## Timestamp decision

Today the bank binds `DateTime<FixedOffset>` at `+09:00`, so Oracle stores `+09:00`. Some SQL formats such columns without `AT TIME ZONE` (for example `TO_CHAR(p.registered_at, 'YYYYMMDDHH24MISS')` in the payee SQL, and the tests' `close_backup.sql` with `TZH:TZM`), so the stored offset is visible. Columns filled by `DEFAULT SYSTIMESTAMP` store the database server's offset; that does not change.

- **Writes:** the adapter binds a `DateTime<FixedOffset>` as `TIMESTAMP` holding its Korean wall time (the value converted to `+09:00` first), and every session runs with `TIME_ZONE = '+09:00'`, so Oracle stores the same instant with offset `+09:00`, as today. `ORA_SDTZ=+09:00` is set with `ENV` in both stages of `bank/Dockerfile` (the integration tests run in the dev stage's image), and every new connection is checked for `SESSIONTIMEZONE = '+09:00'` (see below). No bank SQL uses `CURRENT_TIMESTAMP`, `LOCALTIMESTAMP`, `CURRENT_DATE`, `SYSDATE` or `SESSIONTIMEZONE` (checked by `git grep` on 2026-11-15), so the session time zone changes nothing else.
- **Reads:** a `TIMESTAMP WITH TIME ZONE` value is returned as `DateTime<FixedOffset>` at `+09:00`, the same instant. Values the bank wrote read back exactly as today. A value stored with another offset (a `SYSTIMESTAMP` default) now reads back at `+09:00` instead of its stored offset, with the same instant. This is the one intended difference; if an existing test or contract depends on a read offset other than `+09:00`, stop and report.
- **NULL:** `Option<DateTime<FixedOffset>>::None` binds as a `TIMESTAMP` NULL, the same type as the `Some` path.

## Bounded waiting and the session pool

The driver's `Pool` is not used: its `acquire` cannot time out, its single manager thread opens connections, and returning or closing a pooled connection rolls back with no timeout while holding the pool's mutex. The adapter keeps its own pool of standalone connections. The pool logic lives in its own module, `bank/src/shared/db/pool.rs`, generic over a small connector trait (open, prepare, roll back, close), so it is unit-tested with fake connections that stall, fail or panic, without Oracle. The `oracledb` implementation of that trait is the only part that touches the driver.

1. **State.** One mutex-protected state: idle connections, a `slots` count, and a `closed` flag; plus a `tokio::sync::Notify`. A slot is taken when a connection starts opening and is held while it is open, checked out, idle **or being closed**, so open, opening and closing connections together never exceed `max_sessions`, and neither do the threads blocked in the driver. The mutex is never held across a driver call, and every lock recovers from poisoning (`unwrap_or_else(|e| e.into_inner())`).
2. **Acquire.** `Db::run` keeps one budget, `acquire_timeout` (`SALTATION_DB_ACQUIRE_MS`, default 200 ms), and loops until it runs out: take an idle connection; else, if a slot is free, take it and start an opener; else wait on the `Notify` for the rest of the budget (the `Notified` future is pinned and `enable`d before the mutex is released, so no wake-up is lost). Openers, returns and finished closes all notify. When the budget runs out the caller gets `DbError::Busy`, as today. The tokio `Semaphore` is no longer needed: slots bound the work.
3. **Open.** An opener runs on a dedicated `std::thread` (created with `std::thread::Builder`; if creating it fails, the slot is released and the caller gets an error), never on tokio's blocking pool, so a stuck open cannot delay runtime shutdown. It connects with `Config::set_stmtcachesize(stmt_cache_size)` and `set_transport_connect_timeout(2 s)`, sets `Connection::set_call_timeout` to `DbConfig::call_timeout` (default 30 s, shorter in tests) immediately, then reads `SESSIONTIMEZONE`; unless it is `+09:00`, the connection is closed and the open fails with an error. A finished open always goes to the idle list (or, if `closed`, is closed), never handed directly to the caller that started it, so a caller that timed out loses nothing.
4. **Work.** The unit of work runs on a blocking thread with the connection, inside `std::panic::catch_unwind`.
5. **Return or retire.** If the work returned `Ok`, the adapter rolls back (under the call timeout) and the connection goes back to the idle list. If the work returned an error, hit the call timeout, panicked, or the rollback failed, the connection is **retired**: it is closed on a dedicated thread (Oracle rolls back an open transaction at logoff) and its slot is freed only when that close returns. If that thread cannot be created, the connection is closed on the current blocking thread before the slot is freed. A retired connection is never reused. A panic is resumed after the connection has been handed to retirement.
6. **Close.** `Db::close` sets `closed`, retires every idle connection, wakes all waiters (who get an error), and returns without waiting for the closes; connections that come back or finish opening afterwards are retired. `Db::close` is called on shutdown as today; the same happens when the last clone of a `Db` is dropped (the `Drop` is on the shared inner state, not on each clone).
7. **Warm-up.** `Db::connect` opens `min_sessions` connections through the same openers and waits for them with a 30 s overall deadline (`WARM_UP_WAIT`, settable in `DbConfig` for tests); on timeout start-up fails and the `Db` is closed.
8. **Migrations.** `Db::migrate` opens one standalone connection the same way (call timeout set, `+09:00` checked), then clears the call timeout for the migration, and afterwards retires it like any other connection; it never enters the pool.
9. **Residual limit.** The driver offers no deadline for login or logoff. A database that accepts TCP but never answers them keeps the opening or closing thread blocked, and its slot taken, until the operating system ends the socket. Callers still get `Busy` within the budget, idle connections keep serving, at most `max_sessions` threads are ever blocked in the driver, and process exit is not delayed. Getting those slots back in that case is not promised; the PR records it as a driver limitation to raise upstream.

## Files and components that change

- `bank/tests/driver_parity.rs` (new): driver behaviours the bank depends on, passing on both drivers, with test-only SQL under `bank/tests/sql/`.
- `bank/src/shared/db/pool.rs` (new) with unit tests: the session pool, generic over a connector trait.
- `bank/tests/db_pool.rs` (new): the pool against the real database (added with the new adapter).
- `bank/tests/driver_bench.rs` (new, `#[ignore]`): the A/B workload.
- `scripts/dev/it-oracle.sh`: targets `driver-parity` and `db-pool` (both added to `all`; `db-pool` also runs one case with `ORA_SDTZ=+00:00`) and `driver-bench` (not in `all`); the reset step grants the test user `SELECT` on `V_$SQL`, `V_$SQL_PLAN`, `V_$SQL_PLAN_STATISTICS_ALL` and `V_$SESSION` (test schema only, after every reset) so the parity test can read execution plans.
- `checks.json`: no new check; `oracle-integration` runs `all`, which then includes `driver-parity`.
- `bank/Cargo.toml`, `bank/Cargo.lock`: `oracle` removed, `oracledb = "=26.0.0-beta.6"` added (default features; no `arrow`).
- `bank/src/shared/db/mod.rs`, `bank/src/shared/db/migrations.rs`: the adapter on the new driver.
- `bank/src/shared/ledger/mod.rs`, `bank/src/features/outbox_relay/mod.rs`: import `ToSql` from `crate::shared::db`.
- Other files under `bank/src/**`: only if a call site cannot compile against the adapter; each such edit is listed in the PR. No SQL file changes.
- `bank/Dockerfile`: Instant Client and `libaio` removed from both stages; `ENV ORA_SDTZ=+09:00` in both; comments updated.
- `changes/oracledb-driver/spec.md`: Dependencies and Risks name the new driver.

## Order of work

1. **Parity tests on the old driver.** Add `bank/tests/driver_parity.rs`, using the existing harness (`bank/tests/common`) and the public adapter. Each case must pass on the current driver:
   - a VARCHAR2-keyed lookup (`account` by `account_no`), bound once from `String` and once from `&str`, uses the index: its plan from `dbms_xplan.display_cursor` (`sql_id` and child from `v$sql` by a unique SQL comment) shows `INDEX UNIQUE SCAN` or `INDEX RANGE SCAN` and no `SYS_OP_C2C` in its predicates;
   - a `DateTime<FixedOffset>` at `+09:00` with microseconds, one just before and one at 00:00 KST, written to a `TIMESTAMP(6) WITH TIME ZONE` column: `TO_CHAR(col, 'YYYY-MM-DD HH24:MI:SS.FF6 TZH:TZM')` shows the Korean wall time, the microseconds and `+09:00`, and the value reads back as the same instant at `+09:00`;
   - a value bound at `+00:00` is stored as the same instant (any offset accepted on the old driver, `+09:00` on the new one) and reads back as the same instant;
   - a row filled by `DEFAULT SYSTIMESTAMP` reads back as an instant within 60 s of the test's clock;
   - `NaiveDate` round-trips through a `DATE` column;
   - typed NULLs (`Option::<&str>::None`, `Option::<i64>::None`, `Option::<DateTime<FixedOffset>>::None`) insert NULL and read back as `None`, including alternating `None` and `Some` in successive executions of one statement and in one `execute_batch`;
   - `insert_returning_id` returns the generated ID for a statement whose other binds include a string, a number and a timestamp;
   - `execute_batch` with 3 rows inserts 3 rows; a batch whose third row violates a unique key fails with `ora_code() == Some(1)`, and after `rollback` none of its rows exist;
   - `query_opt` on no row returns `Ok(None)`; a unique-key violation gives `ora_code() == Some(1)`;
   - `rollback` discards and `commit` keeps a write, as seen from a second session;
   Proof: `scripts/dev/it-oracle.sh driver-parity` passes on the old driver. A case the old driver cannot pass as written is reported, not weakened.
2. **Benchmark on the old driver.** Add `bank/tests/driver_bench.rs` (`#[ignore]`), run by target `driver-bench`, which:
   - builds with `cargo test --release`, runs with `SALTATION_DB_ACQUIRE_MS=200` (not the integration suite's 10 s) and the same `docker run --cpus` and `--memory` for both drivers (stated in the output);
   - uses its own initialisation, not the shared test harness's log capture: logs at `warn` to stderr with nothing retained in memory, and a Prometheus recorder installed so statement histograms exist; histogram values are taken as the difference between snapshots at the start and end of the measured window;
   - before each measured run, resets the schema (`migrations`), seeds 1,000 customers with funded accounts and 90 days of history per account (fixed seed), so state is identical across runs; transfers are 1,000 won, and the seed keeps every sender below its daily limit, so business rejections must be zero (asserted);
   - runs each workload in its own process (one `cargo test` invocation per run) so peak RSS is per run: account lookup, balance inquiry, 3-month history (first page), single transfers, random transfers at concurrency 8, 32 and 64, 200 concurrent duplicate replays of one transfer, and the outbox relay draining a backlog of 5,000 events created right before the measured drain;
   - for each run: 30 s warm-up, 60 s measured (the outbox run until drained), and prints one JSON line with successes, non-business errors, `Busy` count, TPS, p50/p95/p99 latency, slowest statement p95 from `bank_db_statement_duration_seconds` over the measured window only, peak RSS and thread count;
   - asserts conservation (sum of balances unchanged), the duplicate moved money once, and the completed-work counts.
   Three runs per workload and concurrency. Run on the OCI host with nothing else using the `saltation-it` project.
   Proof: `driver-bench` exits 0 on the old driver; JSON saved under `.evidence/`.
3. **Adapter on `oracledb`.** In `bank/src/shared/db`:
   - the adapter's own `ToSql` for `i32`, `i64`, `u32`, `u64`, `f64`, `String`, `&str`, `NaiveDate`, `DateTime<FixedOffset>`, `Option<T>` of each (typed NULL of the same type as `Some`), and references to them. No `bool`; if the bank binds one, stop and report;
   - the adapter's own row trait for single values and tuples up to the widest tuple in the bank, over the same value types, reading timestamps as described under "Timestamp decision";
   - timestamps, NULLs and session time zone as described under "Timestamp decision";
   - the adapter-owned session pool, acquisition, call timeout, return/discard guard, warm-up and migrations as described under "Bounded waiting and the session pool";
   - `insert_returning_id`: the SQL ends with `RETURNING <col> INTO :id` (true for all six statements); bind `params` positionally followed by a typed `i64` NULL for `:id`, execute, and take the single value from `returned_data()`; zero or several values remain an error as today;
   - `execute_batch`: map `&[Vec<&dyn ToSql>]` to the driver's batch parameters and call `Connection::execute_batch`; a failing row fails the call;
   - remove the `Bind` wrapper only if step 1's plan case passes without it;
   - `DbError` keeps its variants, now holding `oracledb::Error`; `ora_code()` uses `db_error_code()`; `query_opt` maps `ErrorKind::NoDataFound` to `None`;
   - update the two imports in `ledger` and `outbox_relay`.
   - `ENV ORA_SDTZ=+09:00` in both stages of `bank/Dockerfile` in this step, since the integration tests run in the dev image and the new adapter refuses other session time zones;
   - pool tests. Unit tests in `pool.rs` with fake connections: no more than `max_sessions` slots in use under any mix of idle, opening, checked-out and closing connections; a caller waits for a returned connection within its budget instead of failing; an opener that finishes after its caller timed out puts the connection in the idle list; failed and stalled opens, stalled closes (the slot stays taken until the close returns) and a thread-creation failure; a panicking unit of work retires its connection and the panic reaches the caller; an open that fails after its caller timed out frees its slot; a failing rollback after `Ok` retires the connection; a failing retirement-thread creation still frees the slot; `close` with idle, checked-out and opening connections returns at once, wakes waiters, and retires late arrivals. `bank/tests/db_pool.rs` against Oracle: a unit of work that panics after an `UPDATE` is retired, and an independent `Db` then updates the same row within 2 s and sees the committed value unchanged; after a call timeout (`DBMS_SESSION.SLEEP` beyond a short test call timeout) the same `Db` serves `max_sessions` concurrent units; with `max_sessions = 1` and a 100 ms budget a second `run` gets `Busy` within 1 s; against a TCP listener that accepts and never answers, `Db::connect` fails within the warm-up deadline plus 5 s, a `run` with `min_sessions = 0` gets `Busy` within the budget, and the test process exits normally; `Db::close` on a warmed pool returns within 1 s and a child process that closes a warmed pool exits within 5 s; and, in a separate process started with `ORA_SDTZ=+00:00`, both a `run` with `min_sessions = 0` and `Db::migrate` fail with the time zone error;
   Proof: `scripts/dev/ci-rust.sh clippy` and `ci-rust.sh test` exit 0 (including the `pool.rs` unit tests); `it-oracle.sh driver-parity` and `it-oracle.sh db-pool` pass on the new driver.
4. **Image without Instant Client.** Remove `oracle-instantclient-*` and `libaio` from `bank/Dockerfile`, keep `gcc` only if the build still needs it. Record both images' sizes.
   Proof: `scripts/dev/it-oracle.sh all` passes, then `it-oracle.sh outbox-outage` passes (it is not part of `all`); `ldd` on the bank binary in the runtime image lists no Oracle library.
5. **Benchmark on the new driver.** Run `driver-bench` exactly as in step 2. Then run the paced comparison below on both drivers.
   Proof: JSON saved under `.evidence/`; the PR shows both runs side by side.
   **Paced comparison (added 2026-11-16).** The step 2 windows are closed-loop: each worker sends its next request as soon as the last one answers, so a faster driver offers more load and its tail latency is measured at a higher throughput than the baseline's. On the OCI host the new driver's closed-loop p95 was higher at concurrency 8 and above while its throughput was higher, but at an equal fixed rate (one run each) it was lower. Oracle counters over the measured window were identical per request on both drivers, and `log file sync` averaged lower on the new driver. So latency is compared at a fixed rate:
   - `driver-bench` gains two opt-in settings that change nothing when unset: `BENCH_RATE=<requests/s>` (open-loop arrivals at that rate with at most the entry's concurrency in flight; latency measured from each request's scheduled time; extra JSON fields `rate` and `late`, requests started more than 10 ms late) and `BENCH_ORA_SNAP=1` (Oracle `v$sysstat`, `v$session_event`, `v$sesstat` and `log file sync` histogram snapshots taken by the host, as SYSDBA, right before and after the measured window and saved beside `bench.jsonl`).
   - For lookup:8, balance:8, history:8, transfer:8, transfer:32 and transfer:64, the rate is 70% of the baseline's median closed-loop TPS for that entry, rounded down to a multiple of 10. Three paced runs per entry on each driver, with `BENCH_ORA_SNAP=1`.
6. **Full verification.** `python3 scripts/repo.py verify --change oracledb-driver --full` on the OCI host; then `it-oracle.sh migrations` and `it-oracle.sh e2e` pass.

## Tests and proof

- New parity tests (step 1), passing on both drivers; pool unit tests and `db-pool` (step 3).
- The existing Oracle integration suite unchanged and passing, including the 200 concurrent duplicate requests (money moves once), the 32-task random-transfer conservation test, rollback cases, `outbox-outage`, `observability` and `e2e`. No assertion is relaxed.
- Benchmark before and after (steps 2 and 5), with versions, host, resource flags, run counts and raw JSON in the PR.
- Image size before and after, and `ldd` without Oracle libraries.

**Stop rule.** For each workload and concurrency, compare the median of the three new runs with the median of the three baseline runs. Stop and report to the owner, with the numbers, instead of opening the PR if any of:
- a parity or integration test fails on the new driver and the fix needs anything beyond the adapter, the Dockerfile or the scripts in scope;
- median TPS of the closed-loop runs more than 20% lower;
- median p95 or p99 more than 20% higher: in the paced runs for the six entries that have them (step 5), in the closed-loop runs for transfer:1 and outbox:1. Closed-loop p95/p99 for the six paced entries are reported in the PR but do not decide;
- any non-business error (`Busy` counted separately), or a median `Busy` rate above 0.1% of requests or more than 0.1 percentage points above the baseline's. For duplicate:200, where 200 requests contend for one row by design and the baseline itself returns `Busy` for about 15% of them, the decisive checks are that money moves once and there are no non-business errors; its `Busy` rates are reported side by side as inconclusive, not as a pass;
- a median paced run with more than 5% of requests `late`, on either driver: the rate is too close to that driver's capacity, so that comparison is inconclusive and is rerun once at a rate 10 percentage points lower for both drivers.
If the baseline's own three runs differ by more than 20% in p95 or TPS for a workload, that comparison is inconclusive: rerun both drivers for it once; if still inconclusive, report it as such in the PR rather than as a pass.

## Risks and mitigations

- **Beta driver.** Pinned with `=`; upgrades are separate changes. The parity tests pin the behaviours the bank relies on.
- **Timestamp offsets.** Handled as above; the parity tests cover writes, reads, microseconds, midnight and NULLs on both drivers; the start-up check refuses a session time zone other than `+09:00`.
- **Unbounded driver waits** (acquire, login, cleanup). The adapter owns the pool, bounds every caller's wait, and discards broken connections off the request path; the pool unit tests and `db-pool` test it. Login against a silent server stays unbounded inside the driver (residual limit above).
- **NUMBER decoding through strings** in the driver may cost CPU; the benchmark measures it.
- **Thin-mode limits** (no native network encryption, no 10G verifier); the integration suite proves the connection.
- **Hidden call-site differences** (a type bound today that the new trait lacks): the compiler finds them; each is handled in the adapter, not by changing feature logic.

## Rollback or recovery

Revert the merge commit. No schema, data, contract or configuration format changes, so the old image works against the same database. Rows the new driver wrote are stored with the same instants and offsets as the old driver writes.

## Open questions

None.
