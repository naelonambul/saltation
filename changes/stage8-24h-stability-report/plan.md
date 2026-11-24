# Add Stage 8 24-hour stability run report

## Summary

Document the outcome of the 24-hour continuous Stage 8 stability run executed on the primary OCI ARM64 host (`510c6c9`) after merging the PR #35–#42 fixes (including the PR #38 `GLIBC_TUNABLES` OOM fix and PR #41 `/readyz` + process RSS metrics). Per `REVIEW.md` (OTN benchmark-disclosure clause), the committed report records operational stability, memory bounds, container health/restart outcomes, correctness outcomes, and follow-up N-4 sizing findings without publishing measured Oracle throughput, latency, or wait-time figures.

## Files and components that change

| Paths | Work |
| --- | --- |
| `docs/stage8-24h-stability-report.md` | Add an OTN-compliant stability report covering run configuration, 24-hour container/co-tenant health (N-8), `bank` RSS/RssAnon stability post-PR #38, N-5 daily-close/outbox/5xx outcomes, host resource headroom, and N-4 backfill pacing follow-ups. |

## Order of work

1. **Draft the OTN-compliant report.** Create `docs/stage8-24h-stability-report.md` summarizing the 24-hour run window (`2026-11-23T01:57Z`–`2026-11-24T01:59Z`), revision (`510c6c9`), memory bounds (`VmRSS` peak `63.5 MiB / 1 GiB`, `RssAnon` peak `52.0 MiB`), zero OOM/restarts across all 10 containers, zero 5xx/outbox-relay errors/daily-close mismatches, and the N-4 follow-up work (Argon2 N-6 configuration, CPU budget rebalancing, session reuse audit). Confirm no Oracle throughput/latency/wait-time measurements appear in the committed text. Proof: `python3 scripts/repo.py status --change stage8-24h-stability-report`.

## Tests and proof

- `python3 scripts/repo.py status --change stage8-24h-stability-report`
- Documentation-only CI (`control-plane`, `status`, `identity`, `oracle` doc-only skip, `summary`).

## Risks and mitigations

- **OTN compliance (`REVIEW.md`):** Explicitly exclude Oracle throughput (req/s, total request counts), SQL/HTTP latency, and wait-time numbers from the committed markdown and PR body; keep raw measurements in ignored local `.evidence/`.

## Rollback or recovery

Remove `docs/stage8-24h-stability-report.md` if superseded.
