# Final Review — stage6-opening-balance-headroom

- **Reviewed implementation commit:** `0ff426b8e3fb62f51b94161c42810a14792e9ed0`
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only plan review) and AGY Neon (`gemini-4-neon-high`, read-only plan review + 100k-trial Monte Carlo)

## Verification evidence

- `uv run --project generator --frozen pytest generator/tests -q`: 309 passed (`9.61s`, `OK`).
- `uv run --project generator --frozen ruff check generator` & `ruff format --check generator`: passed (`52 files already formatted`).
- `uv run --project generator --frozen lint-imports --config generator/pyproject.toml --no-cache`: `Contracts: 2 kept, 0 broken`.
- `cargo fmt --manifest-path bank/Cargo.toml --check`: passed.
- `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`: passed (0 warnings).
- `cargo test --manifest-path bank/Cargo.toml --locked --tests --no-run`: all integration test binaries (including `tests/generator_scenarios.rs`) compiled cleanly.
- `python3 scripts/ops/check_new_comments.py`: exit 0, no findings.
- `python3 scripts/repo.py status --change stage6-opening-balance-headroom --base customer-session-expiry`: `status: ok`, `readiness=ready`, 9 changed paths within `write_scope`.
- **Deferred pre-merge gate:** Oracle-backed `repo.py verify --change stage6-opening-balance-headroom --full` (including live execution of `bank/tests/generator_scenarios.rs`) will run on PR CI when this stacked branch is pushed in order.

## Pre-implementation review findings incorporated

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Neon + Astra 6.2 | Critical | `10,000,000` KRW opening balance still carried a `0.061%` per-student tail risk under `student <- older` ring fallback across 100k Monte Carlo trials. | **Addressed** in `0ff426b`: set `STAGE6_OPENING_BALANCE = 20_000_000` KRW (`0` failures across 50k trials, `worst_min > 10M` KRW) and added multi-seed 365-day replay regression test in `test_stage6_schedule.py`. |
| Astra 6.2 + Neon | Important | `sole_trader` (`44.2M`/yr) exceeds `working` (`37.2M`/yr) and incoming ring transfers add upper-balance headroom risk for Scenario G `declined_card`. | **Addressed** in `0ff426b`: raised Scenario G `declined_card` to USD `200,000` (`260,000,000` KRW, strictly above theoretical `20M + max_annual_income + max_annual_incoming` bound) and asserted dynamically against `PROFILES`. |
| Astra 6.2 + Neon | Important | `opening_balance` is not hashed in `schedule_digest` or `roster_digest`, so G-free runs could silently resume old `3M` worlds without a workload version bump. | **Addressed** in `0ff426b`: bumped `WORKLOAD_VERSION = 3` in `stage6_identity.py` and updated `test_stage6_run.py`. |

## Conclusion

All pre-implementation review findings are incorporated and verified locally (`309` generator tests + Rust `fmt`/`clippy`/integration compile). Ready for sequential PR push and full CI verification.
