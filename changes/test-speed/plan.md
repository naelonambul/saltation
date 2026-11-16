# Implementation Plan

Status: draft for the owner. Not approved.

## Summary

Cut the Oracle integration test time without removing or weakening any assertion.

The CI `oracle` job has grown from about 1.5 minutes (2026-11-12) to 11-14 minutes (2026-11-15). Of the last job's ~712 s, 584 s is `cargo test` time inside `it-oracle.sh all`. The slowest suites in that job, as first pass + second pass:

| Suite | Time |
|---|---|
| `atm` | 117 + 112 s |
| `backfill` | 90 s |
| `transfers` | 34 + 34 s |
| `password_reset` | 25 + 25 s |
| `outcomes` | 24 + 22 s |

A Mac runs the same suites about 2.4× faster.

Fable, Astra and Hydrogen reviewed this read-only on 2026-11-15 and agreed on the causes:

1. **Argon2id runs unoptimized.** Tests build in the debug profile, where one Argon2id hash (m=19 MiB, t=2, p=1) takes 223 ms, against 13 ms in release. The suites hash a lot: about 170-200 hashes per ATM pass, about 47 per transfers pass and about 42 per password_reset pass. With `opt-level = 3` for `argon2` and `blake2` only, `password_reset` on the Mac went from 10.3 s to 1.2 s.
2. **Contract validators are rebuilt on every call.** `tests/common/mod.rs` re-reads and re-parses the contract YAML and recompiles a JSON Schema validator for every response and event it checks. ATM does this once or twice per HTTP call, and once per outbox row it scans.
3. **Two ATM leak tests scan the whole OUTBOX** (`seeding_leaks_no_pin`, `no_token_or_pin_leaks`). They fail whenever any earlier event carries the number 7391 or 8462. They failed on unchanged code when `it-oracle.sh atm` ran without a schema reset, and the same failure is latent in `all`. This is a test isolation bug, and it also costs time as the outbox grows.
4. **ATM runs twice, both times in live mode.** `atm.rs::live()` switches the one-way business clock to live in every test, so the first pass is already live. The second run proves nothing new. ATM in backfill time is covered by `backfill.rs::atm_sessions_in_business_time`.

**Not changed:**
- Argon2 parameters: spec N-6's configuration stays exercised.
- `--test-threads=1` and the shared schema.
- Workload sizes: the 1,000-transfer and 200-duplicate runs, and the 8-way ATM races.
- The `backfill.rs` timing sleeps.
- The `find_pin` rules and the PIN constants.
- The schema resets.
- The two passes for every suite that reads the business clock.

**Dependency.** This change is based on `main`. If `oracledb-driver` merges first, rebase onto it. Step 5 then also covers that change's `driver_parity` and `db_pool` suites, which follow ATM and therefore also run in live mode in both passes. Re-check both before moving them.

## Files and components that change

- `bank/Cargo.toml`: `[profile.dev.package.*]` optimization entries. Tests inherit the dev profile. The release profile is untouched, and overflow checks and debug assertions stay on for the `bank` crate.
- `bank/tests/common/mod.rs`: cache parsed contracts and compiled validators per process, with a `OnceLock` keyed by contract file and schema name, or by method, path and status. Results are never cached.
- `bank/tests/atm.rs`, `bank/tests/sql/atm_events.sql` (and the sibling event queries if step 4 needs them): an event-ID watermark per test.
- `scripts/dev/it-oracle.sh`: the `all` order (step 5), plus host-load lines at the start and end of `all` (step 1).
- `docs/test-speed.md`: before and after timings.

## Order of work

1. **Measure the baseline.**
   - Record per-test times on the Mac with `RUSTC_BOOTSTRAP=1 cargo test --test <suite> -- --test-threads=1 -Zunstable-options --report-time` for `atm`, `transfers`, `password_reset`, `outcomes` and `backfill`, after a schema reset.
   - Add `uptime` and `docker stats --no-stream` lines at the start and end of `it-oracle.sh all`, so CI logs show host load.
   - Write the numbers to `docs/test-speed.md`.
   - Proof: `scripts/dev/it-oracle.sh all` passes, and the document holds the table.
2. **Optimize the hashing dependencies.**
   - Set `opt-level = 3` for `argon2` and `blake2` under `[profile.dev.package]`.
   - Then try `[profile.dev.package."*"] opt-level = 3` once, measuring both the warm suite time and the cold rebuild (`cargo build --tests --timings`).
   - Keep `"*"` only if it saves at least 30 s per `all` beyond the two-package setting.
   - Proof: `it-oracle.sh all` passes, the same tests pass in each suite, and the timings go in the document.
3. **Cache contract validators.**
   - Proof: `it-oracle.sh all` passes with every contract and envelope assertion still called at the same places.
   - Add one test that a deliberately invalid body still fails the cached validator.
4. **Scope the two leak tests.**
   - Read `SELECT MAX(event_id) FROM outbox` before the test's first action, which is before `live()` in the seeding test. Scan only events with a greater ID.
   - First assert that the test's own expected events are in that set, so an empty scan cannot pass.
   - Response, log, token and hash checks are unchanged.
   - Proof: `it-oracle.sh atm` passes twice in a row without a schema reset. That is the reproduction that fails today. `all` also passes.
5. **Run ATM once.**
   - In `all`, drop `atm` from the first suite invocation and keep it in the second (live) one.
   - Add an assertion at the start of the second invocation's ATM fixture that the clock is already live.
   - This revises step 12 ("Whole run") of the merged `ledger-products` plan. Approving this plan is the owner's decision to do so.
   - Proof: `it-oracle.sh all` passes, and the suite list in its log shows `atm` once.
6. **Connection churn (conditional).**
   - After steps 2-5, time `common::state()` in ATM.
   - Only if it still costs more than 10 s per pass, set a smaller warm session count for tests in `cargo_test`. First check that no pool test depends on the default.
   - Otherwise record "not needed" in the document.
7. **Full verification.**
   - `python3 scripts/repo.py verify --change test-speed --full` on the OCI host.
   - Compare the CI `oracle` job time with the baseline in `docs/test-speed.md`.

## Tests and proof

- No assertion is removed or relaxed, and every suite runs the same tests. ATM's count drops only because the duplicate pass is gone.
- Step 4 has a failing reproduction on the base and a passing one after: `it-oracle.sh atm` twice without a reset.
- Before and after timings for each step are in `docs/test-speed.md`. The target is a CI `oracle` job well under 10 minutes on an idle runner. That is a goal, not a gate: no assertion is traded for time.

## Risks and mitigations

- **Optimizing dependencies hides a dependency's debug assertion.** Only third-party crates are optimized; the `bank` crate keeps debug assertions and overflow checks.
- **The first build after a lock or toolchain change is slower.** It is measured in step 2 and cached in the `saltation-cargo-target` volume.
- **A validator cache keyed wrongly checks against the wrong schema.** The key includes the file and schema or route. The new negative test proves that invalid bodies still fail.
- **A watermark misses events** created in setup before the watermark. The watermark is taken before the test's first request, and the test asserts that its expected events are present.
- **Host noise** (the same job took 6-17 minutes on one day). Timings are taken as the median of three runs, and step 1's load lines show contention.

## Rollback or recovery

Every step is a test, build-profile or script change. Revert the step's commit. Nothing touches the schema, the product code or the release profile.

## Open questions

- Step 5 changes an approved earlier plan's whole-run order (owner decision; approving this plan decides it).
- CI runner contention with the other services on the OCI host is not addressed here. If step 1 shows it dominates, capping those services is an operations decision for the owner.
