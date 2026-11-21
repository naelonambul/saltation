# Add N-1 to N-3 and N-5 transfer load tests

## Summary

Deliver the transfer load-testing suite (`loadtest/`) required by Stage 8 (`spec.md` N-1, N-2, N-3, N-5, N-8, Acceptance Criterion #7). Provide a self-contained Python 3 (`httpx` + `asyncio`) load-test harness that seeds and authenticates synthetic sender/receiver pools, drives open-loop constant-rate transfer workloads against the bank API, computes server-side and client-side percentiles plus Prometheus `/metrics` deltas (row-lock wait, DB pool saturation, transfer outcomes), verifies post-run N-5 correctness and N-8 co-tenant health, and writes all measured performance reports strictly under the git-ignored `.evidence/loadtest/` directory (never committed to git, honoring the OTN benchmark-disclosure policy in `REVIEW.md`).

## Files and components that change

| Path | Change |
| --- | --- |
| `loadtest/pyproject.toml`, `loadtest/uv.lock`, `loadtest/Dockerfile` | Python 3.13 package (`saltation-loadtest`) with `httpx`, `ruff`, `pytest`, `pytest-asyncio`, matching `generator/` conventions. |
| `loadtest/saltation_loadtest/**` | Open-loop rate-paced runner (`runner.py`), Prometheus histogram & counter delta parser (`metrics.py`), N-1 random-pair / N-2 hot-account / N-3 stepped-saturation scenarios (`scenarios.py`), N-5 correctness & N-8 co-tenant verifier (`verify.py`), and CLI entrypoint (`cli.py`). |
| `loadtest/tests/**` | Unit tests for open-loop pacing, Prometheus histogram quantile delta calculation, pass/fail threshold evaluation (N-1/N-2/N-3/N-5), and report serialization without requiring a live Oracle database. |
| `scripts/dev/ci-loadtest.sh`, `checks.json` | Containerized lint (`ruff`) and unit-test (`pytest`) runner registered in `checks.json` under the `python` group for `loadtest/**`. |
| `infra/compose/compose.yml` | Add an on-demand `loadtest` service under the `tools` profile connected to `internal` and `obs` networks (to reach `bank:8081` and `bank:9100/metrics`) with a 1 GB memory / 1 vCPU budget and `.evidence/loadtest` report mount. |

## Order of work

1. **Create `loadtest/` package and Prometheus metrics/threshold modules** (`loadtest/pyproject.toml`, `loadtest/saltation_loadtest/metrics.py`, `loadtest/saltation_loadtest/thresholds.py`, `loadtest/tests/test_metrics.py`, `loadtest/tests/test_thresholds.py`).
   - Parse Prometheus text exposition from `bank:9100/metrics` before and after each workload window to compute exact server-side transfer latency percentiles (`http_request_duration_seconds` / transfer route), row-lock wait p95, DB pool acquire wait / in-use saturation, and transfer outcome counts.
   - Implement strict threshold evaluators for:
     - **N-1**: sustained 50 TPS for 10 min between random in-bank accounts (`p50 <= 50 ms`, `p95 <= 300 ms`, `p99 <= 1000 ms`, row-lock wait `p95 <= 50 ms`, non-business-rejection success `>= 99.9%`).
     - **N-2**: sustained 50 TPS for 5 min into 1 hot account from `>= 200` sender accounts (`p95 <= 500 ms`, non-business-rejection success `>= 99.9%`).
     - **N-3**: stepped load of 25, 50, 100, 200 TPS (3 min per step); first step whose `p95 > 300 ms` must be `> 100 TPS`, and the limiting resource (lock wait, DB pool, CPU/SQL) is recorded in the `.evidence/loadtest/` report.
   - Proof: `scripts/dev/ci-loadtest.sh lint && scripts/dev/ci-loadtest.sh test`.

2. **Implement open-loop load runner, setup, N-5/N-8 checks, and CLI** (`loadtest/saltation_loadtest/client.py`, `loadtest/saltation_loadtest/runner.py`, `loadtest/saltation_loadtest/scenarios.py`, `loadtest/saltation_loadtest/verify.py`, `loadtest/saltation_loadtest/cli.py`, `loadtest/tests/test_runner.py`).
   - Pre-seed and pre-authenticate >= 250 customers with sufficient balances before timed measurement begins so Argon2 login cost does not contaminate transfer latency percentiles.
   - Use an open-loop scheduler (fixed target inter-arrival schedule, bounded in-flight concurrency) with unique idempotency keys (`bank_tran_id`) and `X-Request-Id` ULIDs so coordinated omission is avoided.
   - Support `--smoke` / configurable `--duration` and `--steps` for quick verification as well as full spec-compliant N-1 (10m), N-2 (5m), and N-3 (4x3m) runs.
   - Write JSON + Markdown evidence reports strictly to `.evidence/loadtest/<timestamp>-<scenario>.{json,md}`.
   - Proof: `scripts/dev/ci-loadtest.sh test` (mocked async HTTP transport tests verifying open-loop pacing, error classification, and report generation).

3. **Wire CI checks and Compose `tools` service** (`loadtest/Dockerfile`, `scripts/dev/ci-loadtest.sh`, `checks.json`, `infra/compose/compose.yml`).
   - Register `loadtest-lint` and `loadtest-test` in `checks.json` under `python` group scoped to `loadtest/**`.
   - Add `loadtest` service to `infra/compose/compose.yml` under `profiles: [tools]`.
   - Proof: `python3 scripts/repo.py status --change stage8-loadtest` and `python3 scripts/repo.py verify --change stage8-loadtest --full`.

4. **Execute live N-1, N-2, N-3, and N-5 load runs on OCI** (evidence stored in `.evidence/loadtest/`, never committed).
   - Run full N-1, N-2, and N-3 against the live OCI `saltation` stack within its N-7 budget and verify N-5 (zero daily-close mismatches, zero duplicate movements, zero F-21 violations) and N-8 (zero restarts on Saltation and co-tenant containers).
   - Report pass/fail outcome only in public text/PRs; keep all measured latency/TPS/wait numbers exclusively inside `.evidence/loadtest/`.

## Risks and mitigations

- **Coordinated omission in client-side load generation.** Use an open-loop arrival schedule and cross-check against server-side Prometheus histograms from `bank:9100/metrics`.
- **Accidental disclosure of Oracle benchmark numbers.** Write all reports only into git-ignored `.evidence/loadtest/` and never commit measured latency, throughput, or wait statistics.
- **Co-tenant impact (N-8) during 200 TPS saturation step.** Cap the `loadtest` container to 1 vCPU / 1 GB RAM and record co-tenant container restart counts and health before and after each run.

## Rollback or recovery

Remove `loadtest/`, `scripts/dev/ci-loadtest.sh`, the `checks.json` entries, and the `tools`-profile `loadtest` service from `infra/compose/compose.yml`. No database schema or production bank behavior is altered.

## Open questions

None.
