# Plan: Record OCI Sizing, OWASP Argon2id Analysis, and Stage 6 Bottleneck Report

## Goal

Record the completed OCI Ampere A1 sizing investigation, pure-Rust Argon2id OWASP minimum configuration analysis (keeping the current OWASP default `19456/2/1` unchanged, with no new parameter adoption and non-OWASP profiles rejected), Candidate A static container budget evaluation, and Stage 6 backfill secondary bottleneck diagnosis in `docs/stage8-oci-sizing-report.md`, strictly complying with the `REVIEW.md` OTN benchmark-disclosure clause.

## Scope

- `docs/stage8-oci-sizing-report.md` (new supporting documentation file)
- `changes/stage8-oci-sizing-report/` (change packet metadata and plan)

## Order of work

1. Author `docs/stage8-oci-sizing-report.md` covering:
   - Explicit OTN compliance notice (`REVIEW.md`): no measured Oracle Database throughput, latency, wait-time, or execution-plan statistics in committed files or PR text.
   - OCI Ampere A1 host & block volume specifications and N-7 runtime budget verification (`check_compose_budget.py` PASS after adding `0.15 vCPU / 128m` to `cloudflared-oci-vpc`).
   - Pure-Rust `argon2 0.5.3` ARM64 microbenchmark results across OWASP Password Storage Cheat Sheet minimum configurations (`46080/1/1`, `19456/2/1` current default, `12288/3/1`, with lower-memory OWASP equivalents `9216/4/1` and `7168/5/1` listed for reference only and non-OWASP profiles explicitly rejected; no default change adopted), plus the mathematical constraint between OWASP Argon2id cost, N-7 (`<= 4 vCPU`), current `38.7%` auth request share, and N-4 (`>= 200 ops/s`, 365 days `<= 24h`).
   - Candidate A static container allocation (`4.00 vCPU / 13.96 GB`) and high-level pass/fail conformance outcomes for exploratory N-1/N-2/N-3 probes (raw figures kept in ignored local `.evidence/` only).
   - Stage 6 backfill secondary bottleneck diagnosis (`generator` CPU saturation and `mongo` cgroup throttling driven by per-wave `BackfillClock.gate`/`move_clock`/`save` and `Stage6Journal._lock` serialized Mongo round-trips and `WriteConcern(w=1, j=True)` fsyncs).
   - Recommended follow-up changes (Stage 6 journal/wave batching, session-reuse vs N-4 target decision, locking static container limits, Prometheus `2GB` retention, and Oracle `MAXSIZE 32G`).
   - Proof: `python3 scripts/repo.py status --change stage8-oci-sizing-report && python3 scripts/repo.py verify --change stage8-oci-sizing-report --full`.
2. Review `docs/stage8-oci-sizing-report.md` against `REVIEW.md` (especially the OTN clause) before committing and opening the PR.

## Risks and rollback

- **Risk:** Accidentally leaking Oracle Database throughput, latency, wait-time, or SQL plan numbers into `docs/stage8-oci-sizing-report.md` or the PR body.
  - **Mitigation:** Include only pure-Rust Argon2id microbenchmarks, container CPU/RAM limits, Mongo/generator code-level bottleneck counts, and high-level pass/fail outcomes; verify with a dedicated grep pass before commit.
- **Rollback:** Revert the single documentation commit; no runtime code or configuration is modified.
