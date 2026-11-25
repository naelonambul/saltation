# Change Plan: `stage8-oci-validation-de0d055`

## Goal

Update `docs/stage8-oci-sizing-report.md` with the post-hardening OCI validation outcomes on `main` @ `de0d055` (following PR #52–#60: `Argon2Pool` + `ClockCache`, Candidate A Compose guardrails, Stage 6 365-day resilience & traffic quality, rollover batching, `bank` P1 hardening, F-27a daily midnight maintenance window, and F-32 normal mobile session reuse), strictly respecting the `REVIEW.md` OTN clause (outcomes and container resource headroom only; raw Oracle-backed latency/wait numbers remain in ignored `.evidence/loadtest/`).

## Order of Work

1. Add **Section 6: Post-Hardening OCI Validation (`de0d055`, PR #52–#60)** to `docs/stage8-oci-sizing-report.md`:
   - Summarize `saltation-loadtest all` (N-1 / N-2 / N-3 exploratory run `20261125T124225Z-f5d30baecaf7`): all N-1/N-2/N-3 thresholds met, `100.000%` non-business success rate, `0` deadlocks, `0` daily-close mismatches, `none_saturated` through the 200 TPS step, `0` container restarts/OOMs (no raw millisecond latency/wait figures published per OTN clause).
   - Summarize the 3-day Stage 6 OWASP `19456/2/1` backfill (`oci-3d-de0d055`): `49m 56s` total wall time (20,000-customer seeding + 3-day backfill), `75,798` succeeded + `200` Scenario G `card.declined` (`0` unresolved, `0` daily-close mismatches, `0` undelivered outbox events), 100% annual quotas (`A..G` + `lookalike`), and container RAM/health (`bank 79.0 MiB / 1 GiB`, `mongo 346.3 MiB / 3 GiB`, `grafana 105.2 MiB / 256 MiB`, `oracle 4.27 GiB / 6 GiB`, `0` restarts, co-tenant `cloudflared-oci-vpc` healthy).
2. Update the follow-up checklist (renumbered to **Section 7**) to mark items #2 (N-4/N-6 alignment + F-32 session reuse, PR #58) and #3 (Candidate A Compose + storage guardrails, PR #53) as completed, leaving formal `loadtest --qualify all`, 365-day backfill + 24-hour live qualification, and Stage 7 Web UI as remaining milestones.

## Verification

- Local:
  - `python3 scripts/repo.py status --change stage8-oci-validation-de0d055`
  - `python3 scripts/repo.py verify --change stage8-oci-validation-de0d055 --group core`
  - Review diff against `REVIEW.md` OTN clause (confirm zero Oracle DB latency/wait/throughput/plan numbers in diff or PR body).
- CI (PR):
  - Required checks green (`control-plane`, `status`, `identity`, `summary`).
