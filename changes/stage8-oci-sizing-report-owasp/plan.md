# Implementation Plan

## Summary

Update `docs/stage8-oci-sizing-report.md` to record the post-PR #46 (`ab742da`, tested at `e3a960a`) 3-day Stage 6 backfill validation results on OCI Candidate A (`4.00 vCPU / 13.96 GB`) across both the exploratory `4096/2/1` A/B comparison and the production-compliant OWASP minimum `19456/2/1` (`concurrency=3`) run, and update the follow-up recommendations to reflect that PR #46's generator/Mongo batching is complete.

All additions strictly obey the `REVIEW.md` OTN clause: only host/container resource limits and utilization, end-to-end Stage 6 backfill duration/completion and HTTP/Argon2id metrics, and pass/fail conformance outcomes are recorded; Oracle Database throughput, latency, wait-time, and execution-plan statistics remain excluded.

## Files and components that change

- `docs/stage8-oci-sizing-report.md` (covered by `change.json` `write_scope`)

## Order of work

1. Add a new Section 5 ("Post-PR #46 Stage 6 Backfill Validation & OWASP `19456/2/1` Results") to `docs/stage8-oci-sizing-report.md` containing:
   - The 3-way comparison table on Candidate A (`20,000` customers, `amplification=20`, 3 days, `workers=8`): Pre-PR #46 (`4096/2/1`) vs Post-PR #46 (`4096/2/1`) vs Post-PR #46 (**OWASP `19456/2/1, concurrency=3`**).
   - Seeding vs backfill breakdown, business ops/s, login share (`33.8%`), Argon2id hash/gate averages, container CPU/memory/throttling observations (excluding Oracle DB latency/throughput/wait-time metrics), and zero-error / zero-mismatch conformance outcomes.
   - Extrapolated 365-day backfill wall-clock estimate (~45–55 hours + ~30 min one-time seeding) under OWASP `19456/2/1`.
2. Update the existing Section 5 (renumbered to Section 6, "Recommended Follow-Up Changes") to mark Stage 6 backfill batching as completed in PR #46 and clarify the remaining two decisions: (a) N-4 vs N-6 alignment (normal-session reuse vs aligning N-4 ops/s with OWASP `19456/2/1` on 4 vCPU) and (b) applying validated Candidate A Compose/storage guardrails before formal qualification.
3. Update Section 1 metadata table to note the post-PR #46 validation revision (`e3a960a`).
4. Review the diff against `REVIEW.md` (specifically the OTN clause) and run `repo.py status` and `repo.py verify --change stage8-oci-sizing-report-owasp --full`.

## Tests and proof

- `python3 scripts/repo.py status --change stage8-oci-sizing-report-owasp` passes with `readiness=ready`.
- `python3 scripts/repo.py verify --change stage8-oci-sizing-report-owasp --full` passes with zero `failed`, `blocked`, or `not-run` checks.
- Manual diff check against `REVIEW.md` confirms zero Oracle Database throughput, latency, wait-time, or execution-plan metrics are present in the document or PR metadata.

## Risks and mitigations

- **Risk:** Accidentally citing Oracle Database internal timing or throughput numbers in a public report.
  - **Mitigation:** Report only container CPU/memory utilization, end-to-end Stage 6 backfill duration/HTTP rates, Argon2id hash/gate timings, and pass/fail conformance counts; explicitly audit the diff before committing.

## Rollback or recovery

Revert the documentation commit; no runtime code, schema, or Compose configuration is changed.

## Open questions

None.
