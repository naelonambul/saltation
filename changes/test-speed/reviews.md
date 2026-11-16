# Reviewer reports (2026-11-15)

Raw read-only reviews behind plan.md. Not approved artifacts.

## Fable (Claude, repository read access)

**Review: Oracle integration test time (read-only; nothing was run, so every saving is an estimate)**

The time is mostly CPU in unoptimized dependency code, repeated per test and per pass, not database waits. I found no accidental long waits: the fixed sleeps in `backfill` total about 7 s and are deliberate timing proofs.

**Ranked changes**

1. **Optimize dependencies in the dev/test profile** — `bank/Cargo.toml`: `[profile.dev.package."*"] opt-level = 3` (minimum: `argon2`, `blake2`).
   - Why: `shared/auth/mod.rs` hashes at 223 ms in debug, and the tests hash a lot. `common::customer` costs 3 hashes, every ATM `start_session` costs 1 (including a dummy verify for unknown accounts), and `transfers::customer(api, 20, …)` costs 22. I count roughly 170–200 hashes per ATM pass and about 50 per transfers pass.
   - Why all dependencies rather than two: the same debug penalty hits the `oracledb` login (PBKDF2-SHA512 in `messages/auth.rs`, 5 connections per `common::state()`) and `serde_yaml` + `jsonschema` (items 4 and 5).
   - Saving in CI: atm about 117 → 30–45 s, transfers 34 → 10–15, password_reset 25 → 3, loans 14 → 4, backfill about −20; roughly 150 s per pass, 250–350 s per job.
   - Confidence: high on direction (matches your 10.3 → 1.2 s), medium on size.
   - Cost and risk: one full dependency rebuild per `Cargo.lock` or toolchain change (a few minutes, cached in `saltation-cargo-target`), and `rust-clippy`/`rust-unit` rebuild once too. `"*"` excludes the `bank` crate, and overflow checks and debug assertions stay on. Release is untouched.

2. **Scope the two leak tests to their own events** — `bank/tests/atm.rs` (`seeding_leaks_no_pin`, `no_token_or_pin_leaks`), `bank/tests/sql/atm_events.sql`.
   - Change: read `SELECT MAX(event_id) FROM outbox` first (primary-key min/max; IDs are monotonic ULIDs from `outbox::next_event_id`), then select `WHERE event_id > :watermark`. Take it before `live()` in the seeding test.
   - Why: today both tests parse the whole OUTBOX, and `checked_event` rebuilds the envelope validator (file read + schema compile) for every `request.completed` row left by all earlier suites.
   - This fixes the standalone failure. It is also latent in `all`: any earlier suite's numeric field equal to 7391 or 8462 fails ATM.
   - Saving: unknown, possibly 5–30 s per pass; it grows with every stage. Confidence: high on correctness, low on size.
   - Trade-off: you lose the accidental check that other suites' events carry no PIN. Each test still drives seeding, session, cash and wrong-PIN paths itself, so nothing designed is lost.

3. **Stop running `atm`, `driver_parity` and `db_pool` twice** — `scripts/dev/it-oracle.sh` (`all`).
   - Why: `atm.rs::live()` posts `/internal/v1/mode/live` in every test, and the switch is one-way. ATM therefore runs in live mode in both passes, and so do the two suites after it. The second run proves nothing about clock mode.
   - ATM in backfill time is already covered by `backfill.rs::atm_sessions_in_business_time`.
   - Saving: about 112 s in CI today, about 35–45 s after item 1. Confidence: high.
   - Risk: this departs from the approved "Whole run" step in `changes/ledger-products/plan.md`, so it needs an owner-approved plan revision.
   - Keep both passes for suites that read the clock: transfers, payees, inbound, password_reset, deposits, loans, accounts, outcomes, listeners. I did not check `auth` and `outbox`.

4. **Cache contract documents and validators** — `bank/tests/common/mod.rs`.
   - Change: put the parsed YAML and compiled validators behind a `OnceLock` keyed by file and schema name in `contract_errors`, `envelope_errors`, `assert_atm_contract`, `assert_atm_response`, `assert_inbound_contract`.
   - Why: each call re-reads and re-parses up to 34 KB of YAML and recompiles the validator; ATM does this once or twice per HTTP call. Same assertions, no weakening.
   - Saving: a few seconds per pass after item 1; more if you skip `"*"`. Confidence: medium.

5. **Cut connection churn** — `bank/tests/common/mod.rs` or the `cargo_test` environment.
   - Why: every `state()` opens 4 warm sessions plus 1 for `migrate()`; ATM does it 29 times per pass.
   - Change: after item 1, measure; if still material, set `SALTATION_DB_MIN_SESSIONS=1` for tests. Do not share one `Db` across tests: that reduces isolation, and `Clock` is tied to each test's runtime.
   - Saving: unknown. Check that no `db_pool` case relies on the default warm count from the environment.

6. **Index-friendly event lookups** (optional, test SQL only) — `atm_request_events.sql` and its siblings.
   - Why: each is a full OUTBOX scan with `JSON_VALUE`, about 80 calls per ATM pass, so cost grows with every stage's events.
   - Change: add `AND event_id > :watermark` (per-test watermark) to the same queries. Low priority once the schema is reset per `all`.

7. **Runner contention**: the same job took 6–17 min on one day, a 2.8× spread on unchanged work, so host noise matters as much as code.
   - Measure by logging `uptime` and `docker stats --no-stream` at the start and end of `it-oracle.sh`.
   - If confirmed, cap CPU for the sonarqube/jenkins containers or schedule their jobs elsewhere.
   - Splitting the oracle group into parallel jobs does not help: `it-oracle.sh` takes a host-wide lock and there is one Oracle instance.

8. **Parallel suites: defer.** Suites share global state: clock mode and business time, the `outcome_refuse` CHECK that `outcomes` adds on OUTBOX, and Mongo `bank_logs`. It would need per-suite schemas, tablespaces and Mongo databases, on 4 shared vCPUs with the Argon2 gate at 2. Revisit only if items 1–3 leave more than about 4 minutes.

**How to measure:** for per-test times on stable, run `RUSTC_BOOTSTRAP=1 cargo test --test atm -- --test-threads=1 -Zunstable-options --report-time`, before and after each item. This splits ATM between hashing and the leak scans. `cargo build --tests --timings` shows the rebuild and link cost.

**Do not change**

- **Argon2 parameters for tests.** A test-only switch means the suite no longer exercises spec N-6's configuration and adds a knob that could reach production. After item 1 it would save only a few seconds.
- **`--test-threads=1`, `FOR UPDATE WAIT 1`, the retry back-offs, `SALTATION_DB_ACQUIRE_MS=10000`.**
- **The 1,000-transfer and 200-duplicate workloads, and the 8-way ATM races.**
- **The timing sleeps and lower-bound asserts in `backfill.rs`** (1 s barrier wait, 1.5 s job, 2 s slow operation).
- **The 365-day advance and the 10 daily-close witnesses.**
- **The `find_pin` boundary rules, and the PIN constants.** Changing the PIN to dodge collisions hides the isolation bug that item 2 fixes.
- **The schema resets before `migrations` and `backfill`.**
- **The live-mode pass for clock-reading suites.**

## Astra (gpt-6-astra, medium)

**About 82% of the measured job is test execution (584/712 s).** Optimize that first. No files were changed, and no builds, tests, containers, or database commands were run. Savings below overlap.

1. **Optimize Argon2 and Blake2 first — high confidence.** [bank/Cargo.toml](/Users/ckh/repos/saltation/bank/Cargo.toml): set `opt-level = 3` for both packages under `profile.dev.package`; tests inherit dev settings. Preserve debug assertions and overflow checks. [Cargo documentation](https://doc.rust-lang.org/cargo/reference/profiles.html#test)
   Password-reset performs approximately **42 Argon2 operations**: reset-code hashing and two operations per valid confirmation, including rejected confirmations—not just password verification. Transfers seeds **29 PINs**, plus nine password hashes and nine login verifications.
   **Saving:** measured password-reset **9.1 s/pass on Mac**; extrapolation suggests **~22 s/pass, ~44 s/CI**. Transfers’ 47 operations imply roughly **10 s/pass locally**, pending measurement. ATM savings remain unmeasured.
   **Risk/cost:** dependency recompilation and relinking once; longer cold compilation, harder dependency debugging. Measure cold-build cost separately from three warm runs. Optimizing all dependencies is a later experiment: wider rebuild cost, unproven incremental benefit.

2. **Cache compiled contract validators — high confidence in redundant work; savings unmeasured.** [common/mod.rs](/Users/ckh/repos/saltation/bank/tests/common/mod.rs): cache parsed contracts and validators per process, keyed by contract/schema or method/path/status.
   Currently every assertion rereads/parses contracts and recompiles schemas; ATM’s `checked_event` does this for **every event returned by whole-OUTBOX scans**. Retain validation of every response/event.
   **Saving:** unknown seconds/pass; approximately twice that in CI, plus backfill. Measure cumulative validator construction versus validation time. **Risk:** incorrect cache keys; never cache assertion results.

3. **Fix leak-test isolation — high confidence.** [atm.rs](/Users/ckh/repos/saltation/bank/tests/atm.rs), [atm_events.sql](/Users/ckh/repos/saltation/bank/tests/sql/atm_events.sql): capture all events newly created during each serial test, including setup, rejection and completion events; assert expected events exist before scanning.
   Prefer a complete before/after event-ID difference over filtering only successful requests or account IDs, which can miss unlocated rejection events. Keep response/log scans, token/hash checks and planted-secret checks.
   **Saving:** unknown seconds/pass, twice in CI; grows with accumulated OUTBOX size. Measure row counts and scan/validation time. **Risk:** incomplete event selection would weaken coverage. Fresh schema alone does not eliminate coincidental numeric matches.

4. **Repair the mode matrix before removing duplicate suites — high confidence.** [it-oracle.sh](/Users/ckh/repos/saltation/scripts/dev/it-oracle.sh), [atm.rs](/Users/ckh/repos/saltation/bank/tests/atm.rs): `live()` explicitly switches the shared clock to LIVE, so ATM already runs live twice and can contaminate subsequent suites’ intended backfill coverage. Assert mode at each suite boundary; inspect actual execution order.
   Keep both modes for auth/accounts/transfers/payees/inbound/password-reset/deposits/loans/outcomes: they exercise business timestamps, expiry, date/limit handling and outcomes under stored versus wall-clock time. Backfill separately proves advancement, rollover, expiry and transition recovery.
   **Saving:** an explicitly live-only ATM run could remove **112–117 s/CI at current timings**, subject to preserving its populated-schema coverage and the approved plan. Listeners and db_pool are mode-independent; driver_parity’s adapter assertions and outbox delivery also merit separation from clock-dependent fixtures. Their savings need measurement.
   **Risk:** merely deleting the second invocation conceals missing mode coverage. Reconcile the existing approved two-pass plan first; do not assume all second-pass work is redundant.

5. **Reduce repeated fixture initialization — medium confidence.** [common/mod.rs](/Users/ckh/repos/saltation/bank/tests/common/mod.rs): measure pool warming, migration checks, clock initialization and router construction separately; consider once-per-process migration initialization for an unchanged schema.
   Each `state()` warms **four connections**, checks migrations, initializes clock rows and commits. Applied migrations are checked, not all rerun. These fixtures use in-process routers, not server startup, and do not connect to MongoDB.
   **Saving:** unknown seconds/pass, doubled in CI; likely most relevant to ATM’s many fixtures. **Risk:** shared pools/state can alter lock and lifecycle tests; retain dedicated fixtures there and retain migration rerun proofs.

6. **Isolate expensive suites before parallelizing — medium confidence.** [runner](/Users/ckh/repos/saltation/scripts/dev/it-oracle.sh), [checks.json](/Users/ckh/repos/saltation/checks.json), [workflow](/Users/ckh/repos/saltation/.github/workflows/repository.yml): use independent Oracle schemas/PDBs and Mongo namespaces, preserving serial tests inside each suite.
   Outcomes repeatedly snapshots **17 tables** and installs schema-wide failure constraints; outbox drains the whole table. Isolation reduces inherited data costs and prevents interference. Backfill needs its own ordered schema.
   **Saving:** unknown; ideal two-way division of 584 s saves at most **292 s/CI before imbalance/setup costs**, not a forecast. Existing host lock, fixed container names and shared Cargo target volume must also be addressed.
   **Risk:** parallel jobs on the same four CPUs can be slower; separate runner capacity is needed, and every shard must remain required by CI.

7. **Measure host pressure and backfill phases — high confidence in need, low confidence in savings.** Runner/workflow plus [backfill.rs](/Users/ckh/repos/saltation/bank/tests/backfill.rs) and [daily_close.rs](/Users/ckh/repos/saltation/bank/tests/common/daily_close.rs).
   Backfill executes hundreds of dates with six registered jobs and repeated queries/commits, plus another **1,024-transfer** concurrency workload. Its deliberate waits total roughly **6 s**, not 90 s. ATM has a deliberate 250 ms lock probe; `WAIT 1` and 10 s acquire limits are ceilings, not fixed delays.
   **Saving:** unknown seconds/pass/CI. Compare identical warm runs on idle versus busy OCI, recording CPU throttling, run queue, I/O and phase timings. Mac’s ~2.4× advantage does not establish contention as the cause. Also account for browser-dates, registered separately in the Oracle group.

**Do not change:**

- Do not reduce transfer counts/concurrency, skip rollover dates, remove rollback snapshots, relax secrecy checks, or shorten timeout assertions simply to save time.
- Reduced Argon2 parameters can serve separately designated functional tests, but replacing these integration runs would lose production-cost/concurrency fidelity. **Not recommended here**; optimization preserves parameters and behavior.
- Do not skip dummy/rejected-path hashing, disable durability, or enable parallel tests against the shared mutable schema.
- Do not remove either clock-mode obligation or Mongo crash/redelivery checks without equivalent proof. The named slow suites do not directly incur Mongo round trips; outbox and runner setup do.
tokens used
89,909
**About 82% of the measured job is test execution (584/712 s).** Optimize that first. No files were changed, and no builds, tests, containers, or database commands were run. Savings below overlap.

1. **Optimize Argon2 and Blake2 first — high confidence.** [bank/Cargo.toml](/Users/ckh/repos/saltation/bank/Cargo.toml): set `opt-level = 3` for both packages under `profile.dev.package`; tests inherit dev settings. Preserve debug assertions and overflow checks. [Cargo documentation](https://doc.rust-lang.org/cargo/reference/profiles.html#test)
   Password-reset performs approximately **42 Argon2 operations**: reset-code hashing and two operations per valid confirmation, including rejected confirmations—not just password verification. Transfers seeds **29 PINs**, plus nine password hashes and nine login verifications.
   **Saving:** measured password-reset **9.1 s/pass on Mac**; extrapolation suggests **~22 s/pass, ~44 s/CI**. Transfers’ 47 operations imply roughly **10 s/pass locally**, pending measurement. ATM savings remain unmeasured.
   **Risk/cost:** dependency recompilation and relinking once; longer cold compilation, harder dependency debugging. Measure cold-build cost separately from three warm runs. Optimizing all dependencies is a later experiment: wider rebuild cost, unproven incremental benefit.

2. **Cache compiled contract validators — high confidence in redundant work; savings unmeasured.** [common/mod.rs](/Users/ckh/repos/saltation/bank/tests/common/mod.rs): cache parsed contracts and validators per process, keyed by contract/schema or method/path/status.
   Currently every assertion rereads/parses contracts and recompiles schemas; ATM’s `checked_event` does this for **every event returned by whole-OUTBOX scans**. Retain validation of every response/event.
   **Saving:** unknown seconds/pass; approximately twice that in CI, plus backfill. Measure cumulative validator construction versus validation time. **Risk:** incorrect cache keys; never cache assertion results.

3. **Fix leak-test isolation — high confidence.** [atm.rs](/Users/ckh/repos/saltation/bank/tests/atm.rs), [atm_events.sql](/Users/ckh/repos/saltation/bank/tests/sql/atm_events.sql): capture all events newly created during each serial test, including setup, rejection and completion events; assert expected events exist before scanning.
   Prefer a complete before/after event-ID difference over filtering only successful requests or account IDs, which can miss unlocated rejection events. Keep response/log scans, token/hash checks and planted-secret checks.
   **Saving:** unknown seconds/pass, twice in CI; grows with accumulated OUTBOX size. Measure row counts and scan/validation time. **Risk:** incomplete event selection would weaken coverage. Fresh schema alone does not eliminate coincidental numeric matches.

4. **Repair the mode matrix before removing duplicate suites — high confidence.** [it-oracle.sh](/Users/ckh/repos/saltation/scripts/dev/it-oracle.sh), [atm.rs](/Users/ckh/repos/saltation/bank/tests/atm.rs): `live()` explicitly switches the shared clock to LIVE, so ATM already runs live twice and can contaminate subsequent suites’ intended backfill coverage. Assert mode at each suite boundary; inspect actual execution order.
   Keep both modes for auth/accounts/transfers/payees/inbound/password-reset/deposits/loans/outcomes: they exercise business timestamps, expiry, date/limit handling and outcomes under stored versus wall-clock time. Backfill separately proves advancement, rollover, expiry and transition recovery.
   **Saving:** an explicitly live-only ATM run could remove **112–117 s/CI at current timings**, subject to preserving its populated-schema coverage and the approved plan. Listeners and db_pool are mode-independent; driver_parity’s adapter assertions and outbox delivery also merit separation from clock-dependent fixtures. Their savings need measurement.
   **Risk:** merely deleting the second invocation conceals missing mode coverage. Reconcile the existing approved two-pass plan first; do not assume all second-pass work is redundant.

5. **Reduce repeated fixture initialization — medium confidence.** [common/mod.rs](/Users/ckh/repos/saltation/bank/tests/common/mod.rs): measure pool warming, migration checks, clock initialization and router construction separately; consider once-per-process migration initialization for an unchanged schema.
   Each `state()` warms **four connections**, checks migrations, initializes clock rows and commits. Applied migrations are checked, not all rerun. These fixtures use in-process routers, not server startup, and do not connect to MongoDB.
   **Saving:** unknown seconds/pass, doubled in CI; likely most relevant to ATM’s many fixtures. **Risk:** shared pools/state can alter lock and lifecycle tests; retain dedicated fixtures there and retain migration rerun proofs.

6. **Isolate expensive suites before parallelizing — medium confidence.** [runner](/Users/ckh/repos/saltation/scripts/dev/it-oracle.sh), [checks.json](/Users/ckh/repos/saltation/checks.json), [workflow](/Users/ckh/repos/saltation/.github/workflows/repository.yml): use independent Oracle schemas/PDBs and Mongo namespaces, preserving serial tests inside each suite.
   Outcomes repeatedly snapshots **17 tables** and installs schema-wide failure constraints; outbox drains the whole table. Isolation reduces inherited data costs and prevents interference. Backfill needs its own ordered schema.
   **Saving:** unknown; ideal two-way division of 584 s saves at most **292 s/CI before imbalance/setup costs**, not a forecast. Existing host lock, fixed container names and shared Cargo target volume must also be addressed.
   **Risk:** parallel jobs on the same four CPUs can be slower; separate runner capacity is needed, and every shard must remain required by CI.

7. **Measure host pressure and backfill phases — high confidence in need, low confidence in savings.** Runner/workflow plus [backfill.rs](/Users/ckh/repos/saltation/bank/tests/backfill.rs) and [daily_close.rs](/Users/ckh/repos/saltation/bank/tests/common/daily_close.rs).
   Backfill executes hundreds of dates with six registered jobs and repeated queries/commits, plus another **1,024-transfer** concurrency workload. Its deliberate waits total roughly **6 s**, not 90 s. ATM has a deliberate 250 ms lock probe; `WAIT 1` and 10 s acquire limits are ceilings, not fixed delays.
   **Saving:** unknown seconds/pass/CI. Compare identical warm runs on idle versus busy OCI, recording CPU throttling, run queue, I/O and phase timings. Mac’s ~2.4× advantage does not establish contention as the cause. Also account for browser-dates, registered separately in the Oracle group.

**Do not change:**

- Do not reduce transfer counts/concurrency, skip rollover dates, remove rollback snapshots, relax secrecy checks, or shorten timeout assertions simply to save time.
- Reduced Argon2 parameters can serve separately designated functional tests, but replacing these integration runs would lose production-cost/concurrency fidelity. **Not recommended here**; optimization preserves parameters and behavior.
- Do not skip dummy/rejected-path hashing, disable durability, or enable parallel tests against the shared mutable schema.
- Do not remove either clock-mode obligation or Mongo crash/redelivery checks without equivalent proof. The named slow suites do not directly incur Mongo round trips; outbox and runner setup do.

## Hydrogen (gemini-4-hydrogen-high, no repository access)

**Note on scope:** [outcomes.rs](file:///Users/ckh/repos/saltation/bank/tests/outcomes.rs) was truncated after ~145 lines in the prompt, `bank/src/**`, `daily_close.rs`, and the other 9 suites were not included, so findings involving app internals (`migrate`, `Db::connect`, `rollover`, `bank/src` validation order) and unseen suites are marked as depending on them.

### Ranked Changes

1. **Compile `argon2` and `blake2` at `opt-level = 3` in dev** — `bank/Cargo.toml`
   - **Change:** Add `[profile.dev.package.argon2]` and `[profile.dev.package.blake2]` with `opt-level = 3` (avoid `[profile.dev.package."*"]` first, which inflates cold builds and strips dep debuginfo).
   - **Expected saving:** ~70–85 s/pass (~140–170 s in CI): ~160–170 Argon2 hashes in `atm.rs` (~35 s/pass), ~46 in `transfers.rs` (~10 s/pass), ~14–40 in `password_reset.rs` (~22 s/pass, matching the 10.3→1.2 s Mac drop), ~32 in `backfill.rs` (~7 s), plus `loans`/`outcomes`/other suites. *(Exact hash counts in invalid-input paths depend on `bank/src` validation order.)*
   - **Confidence & risk:** High confidence (223→13 ms/hash measured). Low risk: only cold-cache compile adds a few seconds (`saltation-cargo-target` caches it); tests still execute real prod Argon2id params.

2. **Run `atm` only in pass 2 (live mode), not pass 1** — [it-oracle.sh](file:///Users/ckh/repos/saltation/scripts/dev/it-oracle.sh#L283-L287)
   - **Change:** Remove `atm` from the pre-`backfill` `cargo_test` list; run it once after `backfill` (or in its own `cargo_test` after `db_pool`).
   - **Expected saving:** ~117 s/run in CI (entire pass 1 of `atm`).
   - **Confidence & risk:** High confidence, zero coverage loss: every DB test in [atm.rs](file:///Users/ckh/repos/saltation/bank/tests/atm.rs#L69-L81) calls `live()`, which posts `/internal/v1/mode/live`, so pass 1 already runs `atm` in live mode and silently flips the shared DB clock to `LIVE` before `driver_parity` and `db_pool`. *(Audit unseen `driver_parity`, `db_pool`, `listeners`, `loans`, `inbound`, and truncated `outcomes.rs` to see if any others are clock-invariant before removing them from pass 1.)*

3. **Cache OpenAPI/JSON schemas and compiled `jsonschema` validators** — [common/mod.rs](file:///Users/ckh/repos/saltation/bank/tests/common/mod.rs#L97-L148)
   - **Change:** Use `std::sync::OnceLock` in `contract_errors`, `envelope_errors`, `assert_inbound_contract`, `assert_atm_contract`, and `assert_atm_response` instead of re-reading YAML/JSON from disk, parsing YAML, rewriting `$ref` strings, and compiling a fresh `draft202012` validator on every single API call and outbox event (hundreds of times per pass in unoptimized debug code).
   - **Expected saving:** ~10–30 s/pass (~20–60 s in CI). **Measure first:** time `atm` or instrument `assert_atm_response`/`envelope_errors` around `cargo test`.
   - **Confidence & risk:** High confidence it eliminates large debug-CPU waste; medium on exact seconds. Zero risk (validators are pure and immutable).

4. **Scope `atm` outbox leak scans to events created by the test** — [atm.rs](file:///Users/ckh/repos/saltation/bank/tests/atm.rs#L228-L250) (`sql/atm_events.sql`)
   - **Change:** Record `MAX(id)` (or timestamp/request IDs) before `seeding_leaks_no_pin` and `no_token_or_pin_leaks` and filter `OUTBOX` to rows created after it, matching how `logs` already slices `&logs[start..]`.
   - **Expected saving:** ~1–5+ s/pass (grows as `OUTBOX` accumulates across suites/runs; **measure** `SELECT COUNT(*) FROM OUTBOX` during `all`).
   - **Confidence & risk:** High confidence: fixes the `7391` cross-run failure without weakening detection of leaks from the test's own actions.

5. **Reuse one `AppState`/`Db` pool per test binary** — [common/mod.rs](file:///Users/ckh/repos/saltation/bank/tests/common/mod.rs#L14-L36), `atm.rs`, `transfers.rs`, etc.
   - **Change:** Initialize `state()` once per binary via `tokio::sync::OnceCell` (cloning `AppState` to override `limits`), rather than running `Db::connect`, `db.migrate()`, and `Clock::init()` in all 26 `atm.rs` tests and every other suite test. Keep `successful_uncommitted_run_releases_session_lock` isolated (it overrides `DbConfig`).
   - **Expected saving:** ~5–20 s/pass (~10–40 s in CI), depending on `DbConfig::min_sessions` and `migrate()` in `bank/src`. **Measure first:** time `common::state()` inside `atm` with `--nocapture`.
   - **Confidence & risk:** Medium confidence; low risk if `AppState` is cloned for per-test `limits` and DB-config tests stay isolated.

6. **Profile and optimize `backfill` rollover work (if still slow after #1)** — [backfill.rs](file:///Users/ckh/repos/saltation/bank/tests/backfill.rs)
   - **Change:** ~77 s of `backfill`'s 90 s CI time is ~390+ simulated rollover days across 6 jobs (`dormancy` 366d, `maturity` 6mo, `live` catch-up) plus `daily_close_tests`. **Measure first:** log `Instant::now()` per phase in `backfill_sequence`; optimize slow SQL/per-day queries in `bank/src/shared/rollover` or `daily_close` (depends on `bank/src` and unseen `common/daily_close.rs`).
   - **Expected saving & risk:** TBD from phase timings; do not reduce simulated days if jobs depend on 365-day/6-month thresholds.

7. **CI host contention vs. suite/job parallelism** — [it-oracle.sh](file:///Users/ckh/repos/saltation/scripts/dev/it-oracle.sh#L22-L32), [checks.json](file:///Users/ckh/repos/saltation/checks.json#L168-L195)
   - **Finding:** CI is consistently ~2.4x slower than the 10 GB Mac VM across every suite (`4 vCPU / 23 GB` shared with `sonarqube`, `jenkins`, `postgres`, `qdrant`, Oracle 19c, MongoDB). **Measure first:** capture `vmstat 1`, `iostat`, and `docker stats` during `it-oracle.sh all`.
   - **Parallelism note:** `it-oracle.sh` serializes per host with `flock`, and suites share one Oracle schema/clock (`outcomes` alters constraints, `backfill` rewinds clock). Splitting CI `oracle` jobs or running parallel schemas on the same 4-vCPU host will either block on `flock` or worsen CPU/memory contention; only split CI jobs if additional runners exist.

### Do Not Change (Would Weaken Tests)
- **Do not lower Argon2id parameters** (`m=19 MiB, t=2, p=1`) in tests: `opt-level = 3` already hits 13 ms/hash, and lower params drop production memory/format verification.
- **Do not skip or weaken contract/envelope checks** (`assert_contract`, `assert_atm_response`, `envelope_errors`) or **`find_pin` / `OUTBOX` leak scans**—cache validators and scope `OUTBOX` queries instead.
- **Do not drop pass 1 or pass 2 for clock-dependent suites** (`transfers`, `deposits`, `loans`, etc.) that actually exercise backfill vs. live modes.
- **Do not shorten `backfill.rs` barrier/cancellation sleeps** (~6 s total) or **reduce `transfers.rs` concurrency/counts** (200 duplicates, 1,000 transfers across 32 tasks).
- **Do not run suites concurrently against one shared Oracle schema/clock.**
