# Plan — stage6-opening-balance-headroom

## Context

During the real 365-day OCI backfill (`oci-365d-live-93fd480`), `saltation-365d-live` aborted on day 248 (`2026-08-03`) with:
`required operation declined for annual-A-121/transfer` (`422` / `bank_rsp_code: 405 잔액이 부족합니다`).

Root cause: `generator/saltation_gen/drivers/stage6_seed.py` seeds every non-dormant Stage 6 customer with a flat `opening_balance = 3,000,000` KRW, while `normal_transfer` / `normal_visit` draws heavy-tailed amounts from `BANKING_KRW_ANCHORS` (mean `147,250`, max `2,000,000` KRW). Over 365 days, `student` customers (`income_krw = 450,000`/month, especially when their ring predecessor is a dormant `C` mule and falls back to `older`) can random-walk below Scenario A's required `1,500,000` KRW debit. A 100k-trial Monte Carlo review (Neon + Astra 6.2) showed that `10,000,000` KRW still has a `0.061%` per-student tail risk (`worst_min = 11,779` KRW), whereas `20,000,000` KRW yields `0` failures across 50k trials (`worst_min > 10,000,000` KRW, `>8.5M` headroom above Scenario A's `1.5M`).

## Implementation steps

1. **Increase uniform Stage 6 opening balance (`generator/saltation_gen/drivers/stage6_seed.py`)**
   - Define `STAGE6_OPENING_BALANCE = 20_000_000` and apply it uniformly to all non-dormant Stage 6 customers (`0 if preparing else STAGE6_OPENING_BALANCE`).
   - Uniform funding across all non-dormant actors introduces no new opening-balance distinction among non-dormant customers (preserving `spec.md` F-32).

2. **Raise Scenario G expected-decline amount above the theoretical upper bound (`generator/saltation_gen/drivers/stage6_steps.py`)**
   - Raise Scenario G `declined_card` from `$50,000` (`65,000,000` KRW) to `$200,000` (`260,000,000` KRW, well within `F-10` max `999,999,999,999` KRW).
   - Even for the highest-income persona (`sole_trader`: `52 × 850,000 = 44,200,000` KRW/yr, empirical 365d peak `~69.8M` KRW at `20M` opening, theoretical upper bound `20M + 44.2M + 68 × 2M = 200.2M` KRW), `260,000,000` KRW strictly exceeds any reachable 365-day + 24h live balance so `declined_card` is guaranteed to decline.

3. **Bump `WORKLOAD_VERSION` (`generator/saltation_gen/drivers/stage6_identity.py` & `generator/tests/test_stage6_run.py`)**
   - Bump `WORKLOAD_VERSION = 3` (from `2`) so a previously seeded `3,000,000`-KRW world (including G-free runs whose `schedule_digest` would otherwise match) cannot silently resume under the new `20,000,000`-KRW workload semantics.
   - Update `generator/tests/test_stage6_run.py` version assertions and add a regression test confirming an old `workload_version = 2` G-free run is rejected on resume.

4. **Update tests and add 365-day headroom regression tests (`generator/tests/test_stage6_seed.py`, `generator/tests/test_stage6_schedule.py`, `bank/tests/generator_scenarios.rs`)**
   - Update `generator/tests/test_stage6_seed.py` (`20_000_000`), `generator/tests/test_stage6_schedule.py` (`200_000`), and `bank/tests/generator_scenarios.rs:1486` (`20_000_000`).
   - Add a regression test in `generator/tests/test_stage6_schedule.py` checking:
     - `STAGE6_OPENING_BALANCE` strictly exceeds the largest single-episode required debit (`1,500,000`) with large headroom, and multi-seed 365-day balance replay of worst-case `student` actors (both `working` and `older` predecessors) stays above `1,500,000` KRW.
     - Scenario G `declined_card` KRW amount (`amount * 1300`) strictly exceeds `STAGE6_OPENING_BALANCE + max(p.income.annual * p.income_krw for p in PROFILES.values()) + max(p.transfer.annual for p in PROFILES.values()) * BANKING_KRW_ANCHORS[-1][1]`.
   - Keep zero new comments/docstrings (`check_new_comments.py`).

## Verification

- `uv run --project generator --frozen pytest generator/tests -q`
- `uv run --project generator --frozen ruff check generator && uv run --project generator --frozen ruff format --check generator`
- `uv run --project generator --frozen lint-imports --config generator/pyproject.toml --no-cache`
- `export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --manifest-path bank/Cargo.toml --check && cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings && cargo test --manifest-path bank/Cargo.toml --locked --tests --no-run`
- `python3 scripts/ops/check_new_comments.py`
- `python3 scripts/repo.py status --change stage6-opening-balance-headroom --base customer-session-expiry`
- Deferred to PR CI after stack merge: full `repo.py verify --change stage6-opening-balance-headroom --full` including Oracle-backed `bank/tests/generator_scenarios.rs` (`scripts/dev/it-oracle.sh`).
- Commit locally on branch `stage6-opening-balance-headroom`.
