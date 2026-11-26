# Plan — stage6-run-identity-v2

## Context

GPT-5.6 Pro audit finding `F-001` and GPT-6 Astra Pro `D-01`: `run_stage6` (`generator/saltation_gen/drivers/stage6.py:493-503`) currently stores only a numeric schema `version: 1`, `seed`, `config`, `run`, `roster_slots`, and clock bounds in `inputs`. It does not bind a run to an explicit `workload_version`, a cryptographic digest of the planned schedule, or planned scenario counts. Resuming an interrupted run after a schedule-code change can therefore misinterpret `backfill_day` and positional `wave_offset` against a different workload.

## Implementation steps

1. **Add canonical schedule identity helper (`generator/saltation_gen/drivers/stage6_identity.py`)**
   - Define `WORKLOAD_VERSION = 2` (owned by scheduling code, separate from manifest schema version).
   - Implement `build_schedule_identity(schedule, planned)` returning:
     - `workload_version`: `WORKLOAD_VERSION`
     - `schedule_digest`: `"sha256:<hex>"` over the full canonical schedule (dormant slots + all episodes and steps, including episode ID, kind, window, carry_in, start timestamp, roles, and each step's name, op, actor, peer, offset, args, `expect_declined`, region, device, foreign_ip)
     - `backfill_schedule_digest`: `"sha256:<hex>"` over dormant slots + all non-live (`window != "live"`) episodes/steps so compatible live-attach can still verify unchanged backfill identity
     - `planned_counts`: deterministic scenario episode counts by window (`backfill`, `live`) and kind, plus total planned scenario operation counts.

2. **Bind Stage 6 manifests to v2 identity (`generator/saltation_gen/drivers/stage6.py`, `generator/saltation_gen/truth/stage6.py`)**
   - Bump `inputs["version"] = 2` in `run_stage6` and embed `workload_version`, `schedule_digest`, `backfill_schedule_digest`, and `planned_counts` from `build_schedule_identity(schedule, planned)`.
   - Record optional `SALTATION_CODE_REVISION` provenance separately on the run document (`provenance.code_revision`) without making harmless non-schedule source edits invalidate checkpoints.
   - In `Stage6Journal.claim()`:
     - Fail closed on mismatched `version`, `workload_version`, `schedule_digest`, or `backfill_schedule_digest` (including legacy v1 manifests lacking identity fields) before accepting ownership or advancing offsets, with a diagnostic message naming stored vs computed versions/digests.
     - Preserve `_compatible_live_attach` only when `version`, `workload_version`, `backfill_schedule_digest`, `planned_counts["backfill"]`, and all non-live inputs match identically.

3. **Add regression tests (`generator/tests/test_stage6_journal.py`, `generator/tests/test_stage6_run.py`)**
   - Identical code/config/schedule resumes cleanly.
   - Changed `workload_version` is rejected before any bank HTTP request with diagnostic naming stored vs computed version.
   - Mutating an episode step, timestamp, wave-relevant field, or `expect_declined` flag changes `schedule_digest` and is rejected before any bank HTTP request.
   - Legacy v1 manifest without `workload_version` / `schedule_digest` fails closed on resume.
   - Compatible live-attach succeeds when `backfill_schedule_digest` and `planned_counts["backfill"]` match, and rejects when backfill identity differs.
   - Pruning completed normal operations does not affect resume identity checks.

## Verification

- Run `uv run --project generator --frozen pytest generator/tests` (all generator tests).
- Run `uv run --project generator --frozen ruff check generator` and `ruff format --check generator`.
- Run `python3 scripts/ops/check_new_comments.py` (verifying zero new comments in changed Python files).
- Run `python3 scripts/repo.py status --change stage6-run-identity-v2 --base document-loopback-trust-topology`.
- Commit locally on branch `stage6-run-identity-v2` (no push while OCI 365-day backfill is active).
