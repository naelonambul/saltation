# Plan: Fix Event Catalog Drift, checks.json Routing, rust-cache Keys, Cargo Cleanup Bound, and Workflow Test Coverage (Packet 2)

## Goal

Resolve the medium-severity agent-pushable CI coverage, contract-drift, caching, and operational hygiene findings from the Carbon + Astra audit:
1. Eliminate contract drift by adding `card.authorized` and `card.declined` to `contracts/codes/event-types.json`.
2. Close `checks.json` routing blind spots (wire `scripts/ops/test_compose_budget.py` into CI, add `scripts/dev/**` to `rust-unit`, add missing mobile/web/contract/auth/transfers/nginx paths to `mobile-browser-dates`, and map docs/policy files to `control-plane`).
3. Harden `scripts/dev/rust-cache.py` (strip `--lib`/`--bins`, separate `clippy` marker key from `test`/`run`, exclude `CARGO_BUILD_JOBS` from `compile_env()`, and fingerprint `tree_parts(bank)` instead of a hardcoded allowlist) with unit tests in `scripts/dev/test_rust_cache.py`.
4. Harden `scripts/dev/it-oracle.sh` (`setup_key` hashes `mongo` image ID + `INIT_SGA_SIZE`/`INIT_PGA_SIZE` + `is_file()` guard, `host_load` samples only `$project` containers) and `scripts/dev/cargo-container.sh` (bound `_cargo_cleanup` retries to 60s instead of looping forever on a wedged daemon).
5. Extend `scripts/tests/test_workflow.py` to enforce SHA-pinned actions, `persist-credentials: false`, and `permissions: contents: read` across all `.github/workflows/*.yml` files (including `weekly-fresh.yml`).

No `.github/workflows/**` files are modified in this packet.

## Scope

- `contracts/codes/event-types.json`
- `checks.json`
- `scripts/dev/rust-cache.py`
- `scripts/dev/test_rust_cache.py`
- `scripts/dev/it-oracle.sh`
- `scripts/dev/cargo-container.sh`
- `scripts/tests/test_workflow.py`

## Order of work

1. **Sync event-type contract catalog (`contracts/codes/event-types.json`)**
   - Add `card.authorized` and `card.declined` descriptions matching `bank/src/features/cards/payment.rs` (`card_payments.rs`).
   - Proof: `python3 -c 'import json; d=json.load(open("contracts/codes/event-types.json"))["event_types"]; assert "card.authorized" in d and "card.declined" in d'`.

2. **Fix `checks.json` routing & wire `test_compose_budget.py` (`checks.json`)**
   - Add `ops-compose-budget-tests` (`python` group, running `python3 -m unittest scripts/ops/test_compose_budget.py`).
   - Add `scripts/dev/**` to `rust-unit.paths`; add `contracts/**` and `scripts/dev/ci-rust.sh` to `rust-clippy.paths`; add `scripts/dev/ci-web.sh` to `web-*.paths`; add `scripts/dev/ci-py.sh` to `py-*.paths`.
   - Add `web/packages/**`, `web/pnpm-lock.yaml`, `contracts/openapi/public.yaml`, `bank/src/features/auth/**`, `bank/src/features/transfers/**`, `web/Dockerfile`, and `infra/nginx/**` to `mobile-browser-dates.paths`.
   - Add `docs/**`, `README.md`, `README.ko.md`, `REVIEW.md`, `LICENSE`, and `.gitignore` to `control-plane.paths` so local routed `repo.py verify` on docs-only changes no longer triggers an unmapped full-suite run.
   - Proof: `python3 scripts/tests/run.py` and `python3 -m unittest scripts/ops/test_compose_budget.py`.

3. **Harden `scripts/dev/rust-cache.py` & `scripts/dev/test_rust_cache.py`**
   - In `_cargo_fingerprint_args()`, also strip `--lib` and `--bins` on `cargo test`.
   - Exclude `CARGO_BUILD_JOBS` from `compile_env()` (parallelism does not affect compiled artifacts).
   - Separate `cargo clippy` into its own marker key (`clippy` subcommand selector) so `clippy` and `test` do not invalidate each other.
   - Replace the hardcoded `bank/` subpath allowlist in `fingerprint()` with `tree_parts(bank)` (`tree_parts` already excludes `.git`, `target`, `__pycache__`, `.env*`, `credentials*`).
   - Add unit tests in `scripts/dev/test_rust_cache.py` covering `--lib`/`--bins` reuse and `CARGO_BUILD_JOBS` invariance.
   - Proof: `python3 -m unittest -v scripts/dev/test_rust_cache.py`.

4. **Harden `scripts/dev/it-oracle.sh` & `scripts/dev/cargo-container.sh`**
   - In `it-oracle.sh`: include `mongo` image ID and `INIT_SGA_SIZE`/`INIT_PGA_SIZE` in `setup_key`, guard `if p.is_file()` in the glob loop, and restrict `host_load()` `docker stats` to containers belonging to `$project` (`--filter "label=com.docker.compose.project=$project"`).
   - In `cargo-container.sh`: bound `_cargo_cleanup()` retries to 60s (300 × 0.2s) instead of an infinite loop, printing a diagnostic message if removal cannot be confirmed.
   - Proof: `bash -n scripts/dev/it-oracle.sh scripts/dev/cargo-container.sh`.

5. **Extend `scripts/tests/test_workflow.py` across all workflows**
   - Add tests verifying every `.github/workflows/*.yml` file pins all `uses:` actions to 40-hex commit SHAs, sets `persist-credentials: false` on every `actions/checkout`, and declares `permissions:\n  contents: read`.
   - Proof: `python3 -m unittest -v scripts.tests.test_workflow` and `python3 scripts/tests/run.py`.

## Risks and rollback

- **Risk 1 (`setup_key` change triggers one-time fresh `saltation-it` volume creation):** Changing `setup_key` causes `it-oracle.sh` to recreate `saltation-it` volumes once on the next run (~15m cold Oracle init) and reuse them thereafter.
  - **Mitigation:** Expected and one-time; ensures the volume key accurately tracks both Oracle and Mongo initialization inputs.
- **Risk 2 (`test_workflow.py` checking `weekly-fresh.yml`):** Verify `weekly-fresh.yml` already satisfies SHA pinning, `persist-credentials: false`, and `permissions: contents: read` before asserting.
- **Rollback:** Revert the single PR commit; no runtime server code or workflow files are modified.
