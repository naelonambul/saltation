# Plan — redact-oracle-backed-benchmark-results

## Context

GPT-5.6 Pro audit finding `F-002`: `REVIEW.md` and the prior owner decision in `changes/redact-bench-numbers/plan.md` keep measured Oracle-backed performance figures (throughput, latency, benchmark-derived durations and extrapolations) in ignored `.evidence/` while tracked public documents report qualitative outcomes, static budgets, methodology, and correctness/stability results only. Two tracked Stage 8 reports still contain measured Oracle-backed end-to-end rates, run durations, and extrapolations.

## Implementation steps

1. **Redact Oracle-backed measured pacing figures in `docs/stage8-24h-stability-report.md`**
   - Replace the measured 24-hour simulated-day throughput/progress rate in Section 5 with a qualitative pacing conclusion and reference to ignored `.evidence/`.
   - Preserve run metadata, PR #38 memory stability metrics, N-8 container health/restarts, N-5/F-36a correctness outcomes, host disk headroom, and planned follow-up changes.

2. **Redact Oracle-backed measured rates, timings, and durations in `docs/stage8-oci-sizing-report.md`**
   - Update the OTN compliance notice to state explicitly that Oracle-backed application throughput, latency, wait-time, execution-plan, and benchmark-derived duration/extrapolation measurements stay in ignored `.evidence/`.
   - Preserve Section 2's standalone pure-Rust `argon2 0.5.3` microbenchmark (explicitly non-Oracle), Section 3's static Candidate A resource budgets and qualitative PASS outcomes, Section 4's architectural bottleneck diagnosis, and Sections 5–6's conformance (`0` mismatches, `0` unresolved, 100% scenario quotas), container memory/health, and run IDs.
   - Remove all Oracle-backed measured wall times, ops/s rates, login rates/timings, and extrapolated completion durations from Sections 5 and 6.2.
   - Leave merged historical packets (`changes/<old-id>/`) and Git history intact per `AGENTS.md` and `changes/redact-bench-numbers/plan.md`.

## Verification

- Manually inspect `git diff` against `REVIEW.md` to confirm zero Oracle-backed measured throughput, latency, wait-time, duration, or extrapolation figures remain in either tracked report.
- Run `python3 scripts/repo.py status --change redact-oracle-backed-benchmark-results --base mobile-transfer-immutable-intent`.
- Run `python3 scripts/oracle_pr_route.py` check (confirming this docs-only packet routes cleanly to `docs-only`).
- Commit locally on branch `redact-oracle-backed-benchmark-results` (no push while OCI 365-day backfill is active).
