# Plan: Harden Oracle CI DB Hygiene, Fix Grafana Readiness Race, and Eliminate Redundant Cargo Cleans

## Goal

Cut `repository/oracle` CI runtime on the OCI Ampere A1 self-hosted runner (~17–22 min → ~8–11 min) without weakening any assertion, test coverage, or security check, while eliminating both recurring CI reliability flakes:
1. Stale MongoDB `truth.*` state surviving a mid-run failure and poisoning subsequent runs (`Stage 6 world credentials already reserved`).
2. `observability` failing with `404 (Grafana dashboard missing)` on fresh `saltation-it_grafanadata` due to Grafana's asynchronous dashboard provisioning.

No `.github/workflows/**` files are modified (100% agent-pushable).

## Scope

- `scripts/dev/rust-cache.py`
- `scripts/dev/test_rust_cache.py`
- `infra/oracle/reset_schema.sql`
- `scripts/dev/it-oracle.sh`

## Order of work

1. **Eliminate `rust-cache.py` self-invalidation across `--test` target lists (`scripts/dev/rust-cache.py`, `scripts/dev/test_rust_cache.py`)**
   - In `fingerprint()`, when `cmd[1] == 'test'`, strip only recognized target-selection arguments (`--test <NAME>`, `--test=<NAME>`, `--tests`, `--no-run`) from the Cargo-side command list before hashing, while keeping all source files, contracts, Cargo manifests/locks, toolchain versions, compiler flags, profile, `--locked`, `--features`, `--target`, and failed-run invalidation strictly intact.
   - Add regression tests in `scripts/dev/test_rust_cache.py` verifying: (a) alternating `--test a` and `--test b` on unchanged sources does NOT trigger `cargo clean`, (b) changing a source file (even with an older mtime) still invalidates and cleans, and (c) a failed run leaves the cache untrusted.
   - Proof: `python3 -m unittest scripts/dev/test_rust_cache.py`.

2. **Airtight Clean-Start & Clean-Exit DB hygiene preserving `saltation-it_oradata` (`infra/oracle/reset_schema.sql`, `scripts/dev/it-oracle.sh`)**
   - In `infra/oracle/reset_schema.sql`: disconnect active `SALTATION` sessions in `ORCLPDB1` and retry `DROP USER saltation CASCADE` on `ORA-01940` (ignoring `ORA-01918`), recreate `saltation`, and apply the 4 `SYS.V_$` grants in the same single `sqlplus` call (halving `sqlplus` spawns per reset).
   - In `scripts/dev/it-oracle.sh` `reset_schema()`: connect to MongoDB as `root` (`--authenticationDatabase admin`) and drop all non-system collections in both `bank_logs` and `truth` plus any orphaned `stage6_verifier_*` users, preserving the permanent `bank`/`generator` users and `saltation-it_mongodata` / `saltation-it_oradata` volumes.
   - Add Clean-Start reaping (remove stale `$cargo_name` and one-off Compose containers, stop app services, ensure `oracle` and `mongo` are healthy, run `reset_schema()` at entry to `all` and `browser-dates`) and an `EXIT` trap (`cleanup_it`) that reaps background/one-off containers, stops app services, and runs best-effort `reset_schema()` on both success and failure while preserving the original exit code.
   - Proof: `bash -n scripts/dev/it-oracle.sh` and `python3 scripts/repo.py status --change ci-oracle-hygiene-speed`.

3. **Fix Grafana provisioning race & remove redundant container/Playwright work (`scripts/dev/it-oracle.sh`)**
   - In `observability()`: pass explicit application/observability services (`bank web alloy loki prometheus grafana`) to `compose up -d --build --force-recreate --wait` so already-healthy `oracle` and `mongo` are NOT force-recreated.
   - Replace the single zero-retry Grafana `curl` check with a bounded retry loop (up to 30 × 2s): retry `404`/connection errors during provisioning, fail immediately on `401`/`403`, and verify HTTP 200 with `"title":"Saltation bank"`.
   - In `browser_dates()`: start only `bank web` (instead of all 8 services including Alloy/Loki/Prometheus/Grafana), run `reset_schema` before seeding, and run both timezones (`Etc/GMT+12`, `Pacific/Kiritimati`) sequentially inside a single Playwright container with a single `pnpm install --frozen-lockfile`, reporting each zone's result independently and avoiding recursive `chown -R` over `node_modules`.
   - Avoid rebuilding `bank` / `generator` images repeatedly across the 5 generator targets within one `it-oracle.sh` invocation via once-per-invocation build guards (preserving standalone target builds).
   - Proof: `bash -n scripts/dev/it-oracle.sh`, `python3 -m unittest scripts/dev/test_rust_cache.py`, and full CI `repository/oracle` run on this PR.

## Risks and rollback

- **Risk 1 (Stale Cargo binary across test targets):** Stripping `--test <NAME>` could mask a real build input if compiler flags or features were stripped too.
  - **Mitigation:** Strip ONLY `--test <NAME>` / `--test=<NAME>` / `--tests` / `--no-run`; keep all other flags, source/contract bytes, and failed-run invalidation; cover with unit tests in `test_rust_cache.py`.
- **Risk 2 (Dropping Mongo collections breaks permanent users):** Using `dropDatabase()` would delete database-scoped `bank`/`generator` users.
  - **Mitigation:** Drop only non-system collections (`!c.startsWith("system.")`) and only `stage6_verifier_*` users, keeping permanent `bank` and `generator` users intact (verified every run by `mongo-users`).
- **Rollback:** Revert the single PR commit; no application code, schema, or workflow files are modified.
