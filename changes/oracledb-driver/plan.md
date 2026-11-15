# Implementation Plan

## Summary

Replace the `oracle` crate (0.6.3, ODPI-C over Oracle Instant Client) with Oracle's pure-Rust driver `oracledb`, pinned to `=26.0.0-beta.6`, and remove Instant Client from the bank image. The owner decided on 2026-11-15 to make this change (issue #10). No SQL, schema, business rule or API contract changes. Every driver call stays inside `bank/src/shared/db`. The adapter keeps its method names and call shapes (`Db::run`, `Session::execute/insert_returning_id/query_row/query_opt/query_all/execute_batch/commit/rollback`, `DbError::ora_code`), but the types in their signatures change from `oracle`'s to the adapter's own: the bind trait (`crate::shared::db::ToSql`), the row trait used by the query methods, and the error held inside `DbError`. Feature code changes only where it names `oracle::` directly (two imports).

Before the switch, the change adds driver parity tests and a benchmark, run first on the old driver to record a baseline, then again on the new driver on the same host against the same Oracle 19.19 EE container.

Facts checked against the `oracledb` 26.0.0-beta.6 source (crates.io, 2026-11-15):
- The API is synchronous (`Pool::acquire`, `Connection::execute/query/query_row/execute_batch`), so the tokio `Semaphore` + `spawn_blocking` boundary stays.
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

The driver's `Pool` is not used: its `acquire` cannot time out, its single manager thread opens connections, and returning or closing a pooled connection rolls back with no timeout while holding the pool's mutex. The adapter keeps its own small pool of standalone connections (`oracledb::connect` with `Config::set_stmtcachesize(stmt_cache_size)` and `set_transport_connect_timeout`), so every wait is under the adapter's control:

1. **Idle list and capacity.** A mutex-protected list of idle connections and a count of connections that exist or are being opened, never above `max_sessions`. The mutex is never held across a driver call.
2. **Acquire.** `Db::run` keeps one budget, `acquire_timeout` (`SALTATION_DB_ACQUIRE_MS`, default 200 ms): take a semaphore permit (max `max_sessions`), then take an idle connection, or, if capacity allows, open one. Opening runs on a dedicated `std::thread` (not tokio's blocking pool, so a stuck open never delays runtime shutdown) and is awaited for the rest of the budget. On timeout the caller gets `DbError::Busy`; the opening thread finishes in the background and puts the connection in the idle list, or gives up its capacity slot on error. Every newly opened connection reads `SESSIONTIMEZONE` and is discarded with an error unless it is `+09:00`, so the check does not depend on warm-up or `min_sessions`.
3. **Work.** The unit of work runs on a blocking thread with the connection and the permit, as today, with a call timeout (`DB_CALL_TIMEOUT`, a 30 s constant) on the connection.
4. **Return or discard.** A guard owned by the blocking task runs whether the work returns or panics (the panic is resumed after the guard): it rolls back under the call timeout; on success the connection goes back to the idle list; on any error, a call timeout or a panic, the connection is discarded. Discarding drops the standalone connection on a detached `std::thread` (the driver's logoff has no timeout) and frees the capacity slot at once, so a broken connection never blocks a request or the pool.
5. **Warm-up.** `Db::connect` opens `min_sessions` connections the same way, with a 30 s overall deadline (`WARM_UP_WAIT`); on timeout start-up fails.
6. **Migrations.** `Db::migrate` uses its own standalone connection with no call timeout, checked for `+09:00` like any other, and closes it afterwards; it never enters the pool.
7. **Residual limit.** A database that accepts TCP but never answers the login keeps each opening thread blocked until the operating system ends the socket; the driver offers no deadline for login. Callers still get `Busy` within the budget, at most `max_sessions` such threads exist, idle connections keep serving, and process exit is not delayed. Recovery of that capacity in that one case is not promised; the PR records it as a driver limitation to raise upstream.

## Files and components that change

- `bank/tests/driver_parity.rs` (new): driver behaviours the bank depends on, with test-only SQL under `bank/tests/sql/`.
- `bank/tests/driver_bench.rs` (new, `#[ignore]`): the A/B workload.
- `scripts/dev/it-oracle.sh`: targets `driver-parity` (added to `all`) and `driver-bench` (not in `all`); the reset step grants the test user `SELECT` on `V_$SQL`, `V_$SQL_PLAN`, `V_$SQL_PLAN_STATISTICS_ALL` and `V_$SESSION` (test schema only, after every reset) so the parity test can read execution plans.
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
   - a unit of work that panics after an `UPDATE` leaves no lock: a second `run` on the same `Db` updating the same row succeeds within 2 s, and the same `Db` still serves `max_sessions` concurrent units afterwards;
   - a unit of work that hits the call timeout (`DBMS_SESSION.SLEEP` beyond a short test call timeout) returns an error, its connection is discarded, and the same `Db` serves the next `run`;
   - with `max_sessions = 1` and a 100 ms acquire timeout, a second `run` while the first holds the session returns `DbError::Busy` within 1 s;
   - a `Db` whose connect string points at a TCP listener that accepts and never answers (started by the test) fails `Db::connect` within the warm-up deadline plus 5 s (the test sets a short deadline through a config field), a `run` on a `Db` with `min_sessions = 0` against it returns `Busy` within the acquire budget, and the test process still exits normally.
   Proof: `scripts/dev/it-oracle.sh driver-parity` passes on the old driver. A case the old driver cannot pass as written (for example the silent listener) is reported, not weakened.
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
   Proof: `scripts/dev/ci-rust.sh clippy` and `ci-rust.sh test` exit 0; `it-oracle.sh driver-parity` passes on the new driver.
4. **Image without Instant Client.** Remove `oracle-instantclient-*` and `libaio` from `bank/Dockerfile`, add `ENV ORA_SDTZ=+09:00` to both stages; keep `gcc` only if the build still needs it. Record both images' sizes.
   Proof: `scripts/dev/it-oracle.sh all` passes, then `it-oracle.sh outbox-outage` passes (it is not part of `all`); `ldd` on the bank binary in the runtime image lists no Oracle library.
5. **Benchmark on the new driver.** Run `driver-bench` exactly as in step 2.
   Proof: JSON saved under `.evidence/`; the PR shows both runs side by side.
6. **Full verification.** `python3 scripts/repo.py verify --change oracledb-driver --full` on the OCI host; then `it-oracle.sh migrations` and `it-oracle.sh e2e` pass.

## Tests and proof

- New parity tests (step 1), passing on both drivers.
- The existing Oracle integration suite unchanged and passing, including the 200 concurrent duplicate requests (money moves once), the 32-task random-transfer conservation test, rollback cases, `outbox-outage`, `observability` and `e2e`. No assertion is relaxed.
- Benchmark before and after (steps 2 and 5), with versions, host, resource flags, run counts and raw JSON in the PR.
- Image size before and after, and `ldd` without Oracle libraries.

**Stop rule.** For each workload and concurrency, compare the median of the three new runs with the median of the three baseline runs. Stop and report to the owner, with the numbers, instead of opening the PR if any of:
- a parity or integration test fails on the new driver and the fix needs anything beyond the adapter, the Dockerfile or the scripts in scope;
- median p95 or p99 is more than 20% higher, or median TPS more than 20% lower;
- any non-business error (`Busy` counted separately), or a median `Busy` rate above 0.1% of requests or more than 0.1 percentage points above the baseline's.
If the baseline's own three runs differ by more than 20% in p95 or TPS for a workload, that comparison is inconclusive: rerun both drivers for it once; if still inconclusive, report it as such in the PR rather than as a pass.

## Risks and mitigations

- **Beta driver.** Pinned with `=`; upgrades are separate changes. The parity tests pin the behaviours the bank relies on.
- **Timestamp offsets.** Handled as above; the parity tests cover writes, reads, microseconds, midnight and NULLs on both drivers; the start-up check refuses a session time zone other than `+09:00`.
- **Unbounded driver waits** (acquire, login, cleanup). The adapter owns the pool, bounds every caller's wait, and discards broken connections off the request path; the silent-listener, call-timeout and panic cases test it. Login against a silent server stays unbounded inside the driver (residual limit above).
- **NUMBER decoding through strings** in the driver may cost CPU; the benchmark measures it.
- **Thin-mode limits** (no native network encryption, no 10G verifier); the integration suite proves the connection.
- **Hidden call-site differences** (a type bound today that the new trait lacks): the compiler finds them; each is handled in the adapter, not by changing feature logic.

## Rollback or recovery

Revert the merge commit. No schema, data, contract or configuration format changes, so the old image works against the same database. Rows the new driver wrote are stored with the same instants and offsets as the old driver writes.

## Open questions

None.
