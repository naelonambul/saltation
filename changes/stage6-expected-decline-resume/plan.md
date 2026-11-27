# Plan — stage6-expected-decline-resume

## Context

GPT-6 Astra Pro audit finding `F-01`: `Stage6Executor._dispatch` checks `step.args.get("expect_declined")` when validating a freshly persisted terminal state (`succeeded` vs `declined`), whereas `Stage6Executor.execute`'s resumed-terminal fast path (`previous["state"] in {"succeeded", "declined"}`) rejects every non-normal `declined` operation without checking `expect_declined`. If a Stage 6 run stops after persisting an expected Scenario G card decline and before saving `wave_offset`, resuming the wave raises `RecoveryError` on a valid terminal operation.

## Implementation steps

1. **Unify terminal-outcome validation (`generator/saltation_gen/drivers/stage6_execute.py`)**
   - Extract `_validate_terminal_outcome(episode: Episode, step: Step, terminal: str) -> None`.
   - Enforce:
     - `expect_declined=True` requires `terminal == "declined"` (raises `RecoveryError("expected declined outcome ...")` otherwise).
     - `expect_declined=False` rejects `terminal == "declined"` for non-normal episodes (`not episode.kind.startswith("normal_")`) with `RecoveryError("required operation declined ...")`.
   - Call `_validate_terminal_outcome` from both `Stage6Executor.execute` (terminal replay fast path) and `Stage6Executor._dispatch` (fresh dispatch).
   - Preserve all stored request IDs, request keys, labels, state transitions, and the rule that terminal operations are never resent.

2. **Add regression tests (`generator/tests/test_stage6_steps.py`)**
   - Test `Stage6Executor.execute` against already-terminal journal records covering:
     - Scenario G + `expect_declined=True` + `declined` → returns without HTTP dispatch;
     - Scenario G + `expect_declined=True` + `succeeded` → raises `RecoveryError`;
     - Scenario G + `expect_declined=False` + `declined` → raises `RecoveryError`;
     - Scenario G + `expect_declined=False` + `succeeded` → returns without HTTP dispatch;
     - `normal_card` + `expect_declined=False` + `declined` → returns without HTTP dispatch.

## Verification

- Run `uv run --project generator --frozen pytest generator/tests/test_stage6_steps.py` (and pure-Python generator test suite).
- Run `uv run --project generator --frozen ruff check generator` and `ruff format --check generator`.
- Run `python3 scripts/repo.py status --change stage6-expected-decline-resume`.
- Commit locally on branch `stage6-expected-decline-resume` without pushing while the OCI 365-day backfill is active.
