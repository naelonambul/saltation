# Implementation Plan

## Summary

Update the root `intent.md` and `spec.md` baseline to establish:
1. The three-environment deployment split: **OCI ARM64 via Docker Compose (`infra/compose/`)**, **local x86-64 Rust-native via Terraform + Ansible (`infra/lab/`, `win-vmware-lab`, Docker-free)**, and **local x86-64 Classic Big-Data comparison (`Apache Kafka`, `Apache Hadoop/HDFS + Iceberg`, `Apache Spark` via `infra/lab/`, `win-vmware-lab`, Docker-free)** for direct downstream CPU and memory comparison against the Rust-native stack.
2. The pure-Rust streaming, lakehouse, and FDS/MLOps target architecture (**Apache Iggy + `iggy-connect`**, **RustFS**, **Lakekeeper + dedicated PostgreSQL**, **Sail**, and **`detect/` with `ort` ONNX**), withdrawing the superseded MongoDB-to-PostgreSQL(JSONB) log-store plan.
3. Phase 1 **Dual-Sink** outbox relay delivery (`bank` -> MongoDB `bank_logs` + Apache Iggy, single outbox path for all F-35 events).
4. The updated OCI `N-7` resource policy: fixed **4 vCPU** concurrent active cap with a **flexible memory ceiling up to the 24 GB physical envelope** for both the core (`compose.yml`) and combined (`compose.yml` + `platform.yml`) stacks, calibrated by OCI test measurements.

This change updates baseline documents only; no runtime code, Compose files, Ansible playbooks, or CI checks are modified in this packet.

## Files and components that change

- `intent.md`: replaced by the approved `changes/rust-lakehouse-fds-baseline/intent.md`.
- `spec.md`: replaced by the approved `changes/rust-lakehouse-fds-baseline/spec.md`.

## Order of work

1. Review and approve `changes/rust-lakehouse-fds-baseline/intent.md`, `spec.md`, and `plan.md` in chain order (`intent.md` -> `spec.md` -> `plan.md`).
2. Copy the approved `changes/rust-lakehouse-fds-baseline/intent.md` and `changes/rust-lakehouse-fds-baseline/spec.md` over root `intent.md` and `spec.md`.
3. Verify with `python3 scripts/repo.py status --change rust-lakehouse-fds-baseline` and `python3 scripts/repo.py verify --change rust-lakehouse-fds-baseline --full`.
4. Review against `REVIEW.md` (including OTN benchmark compliance: zero Oracle-backed measured throughput, latency, wait-time, execution-plan, or benchmark-derived duration figures), record `review.md`, re-verify, and open a PR.

## Expected byproducts and cleanup

- `.evidence/<run>/` (gitignored, repo-owned): verification evidence from `scripts/repo.py verify`. Retain for `review.md`.

## Tests and proof

- `python3 scripts/repo.py status --change rust-lakehouse-fds-baseline`
- `python3 scripts/repo.py verify --change rust-lakehouse-fds-baseline --full` (runs `control-plane`)
- Byte-for-byte equality between root `intent.md`/`spec.md` and `changes/rust-lakehouse-fds-baseline/{intent.md,spec.md}` (`status` checks `carried-unmerged`).

## Risks and mitigations

- Introducing unverified downstream component details into the baseline could over-constrain implementation: keep exact container tags, plugin build recipes, and OCI AJD compatibility framed as staged downstream changes gated by spike verification.
- OTN compliance risk when referencing OCI sizing or telemetry: omit all Oracle-backed measured throughput, latency, wait-time, execution-plan, and benchmark-derived duration figures from `intent.md` and `spec.md`.

## Rollback or recovery

Revert the squash commit on `main`. No code, schema, or infrastructure state changes.

## Open questions

None.
