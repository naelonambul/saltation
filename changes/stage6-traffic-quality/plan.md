# Implementation Plan

## Summary

Fix the three Stage 6 traffic-quality and scenario-coverage issues identified as `P0-7` in the maintainer review:

1. **Emit `card.declined` in Scenario G (`stage6_steps.py`, `stage6_execute.py`, `stage6.py`, `truth/stage6.py`):** Add a high-value overseas card attempt (`declined_card`, `$50,000 USD` = `65,000,000 KRW`, `expect_declined=True`) to `_foreign_card_misuse()` so Scenario G produces both `card.authorized` and `card.declined` as required by `spec.md:123, 415`. Update `Stage6Executor.execute()` and `Stage6Journal.require_success()` to accept `state == "declined"` for steps marked `expect_declined=True` (and fail closed if an `expect_declined` step unexpectedly succeeds).
2. **Spread `normal_episodes()` across the hour (`stage6_schedule.py`):** Replace the fixed `minute = 12 * activity_index` clustering (which concentrated ~2,740 ops into 6 single-minute spikes per hour) with deterministic per-customer/activity minute spreading across `0..57` (`stable_number(seed, customer, kind)`), preserving the 3-minute ring-neighbor separation for `normal_transfer` so adjacent senders never overlap.
3. **Distribute `normal_atm` across all 17 regional `ATM_TERMINALS` (`stage6_execute.py`, `stage6.py`):** Route `normal_atm` episodes to the regional terminal matching the customer's `home_context` region (`TERMINAL_BY_REGION[region]`, falling back to `ATM_TERMINALS[0]`), while keeping scenario ATM steps (`D`, `G`) on `ATM_TERMINALS[0]`. Update `_waves()` in `stage6.py` to serialize ATM steps per terminal (`atm:<terminal_id>` in `subjects`) instead of globally forcing wave size `1`, enabling parallel regional ATM waves without violating `ATM_SESSION_TERMINAL_OPEN_UK`.

## Files and components that change

- `generator/saltation_gen/drivers/stage6_steps.py`: add `expect_declined: bool = False` option to `_card()` and append `declined_card` (`$50,000 USD`, `expect_declined=True`) to `_foreign_card_misuse()`.
- `generator/saltation_gen/drivers/stage6_execute.py`:
  - Honor `step.args.get("expect_declined")` in `execute()` (require `terminal == "declined"` instead of raising on decline).
  - Resolve regional ATM terminal (`TERMINAL_BY_REGION`) for `normal_atm` steps instead of hardcoding `ATM_TERMINALS[0]`.
- `generator/saltation_gen/drivers/stage6.py`:
  - Attach `terminal_id` on `normal_atm` steps (matching the customer's home region) and update `_waves()` to track `f"atm:{terminal_id}"` in `subjects` so distinct terminals execute concurrently while same-terminal steps still serialize.
  - Pass `expected_declined` step names from `_execute_one()` to `journal.require_success()`.
- `generator/saltation_gen/truth/stage6.py`: extend `require_success(episode_id, names, *, expected_declined=None)` to verify `succeeded` steps are `"succeeded"` and `expected_declined` steps are `"declined"`.
- `generator/saltation_gen/drivers/stage6_schedule.py`: spread `normal_episodes()` minutes deterministically across the hour while preserving `normal_transfer` ring-neighbor lane spacing.
- `generator/tests/test_stage6_schedule.py`, `generator/tests/test_stage6_steps.py`, `generator/tests/test_stage6_run.py`, `generator/tests/test_stage6_journal.py`, `bank/tests/generator_lifecycle.rs`: unit and lifecycle-verifier tests verifying Scenario G `declined_card` (7 steps, `state == "declined"` / HTTP 422), hour-wide minute spread, regional ATM terminal wave concurrency vs same-terminal serialization, and `require_success` with `expected_declined`.

## Order of work

1. Add `declined_card` (`expect_declined=True`) in `stage6_steps.py`, handle `expect_declined` in `stage6_execute.py`, and extend `require_success` in `truth/stage6.py` + `_execute_one` in `stage6.py`.
2. Spread `normal_episodes()` minutes in `stage6_schedule.py`.
3. Route `normal_atm` to regional `ATM_TERMINALS` in `stage6.py` and `stage6_execute.py`, and update `_waves()` to serialize per `terminal_id`.
4. Add/update unit tests and run `ruff check`, `ruff format --check`, `pytest`, `repo.py status`, and `repo.py verify --group core`.

## Tests and proof

- `generator/tests/test_stage6_schedule.py`:
  - `normal_episodes()` spreads timestamps across diverse minutes in `0..57` while keeping `normal_transfer` ring neighbors separated.
  - Scenario G steps include `declined_card` with `expect_declined=True`.
- `generator/tests/test_stage6_run.py`:
  - `_waves()` serializes two ATM steps on the same terminal (preserving existing test) and batches two ATM steps on distinct regional terminals into the same wave.
- `generator/tests/test_stage6_journal.py`:
  - `require_success(..., expected_declined=["declined_card"])` passes when `declined_card` has state `"declined"` and fails if it is `"succeeded"` or missing.
- All `generator` unit tests (`220+`), `ruff`, `control-plane`, and OCI `oracle-integration` (`stage6_lifecycle_harness.py`) pass.

## Risks and mitigations

- **Risk:** `normal_transfer` ring neighbors (`index` and `index + 1`) landing within 60s of each other if minutes are randomized independently.
  - **Mitigation:** Assign `normal_transfer` minutes from `3 * lane` + `9 * bucket` (or keep `minute = (bucket * 9 + 3 * lane) % 57` where `bucket = stable_number(seed, day.isoformat(), index // 2) % 6` so adjacent ring pairs share a bucket and stay separated by `3` minutes, while different pairs spread across all 60 minutes!).
- **Risk:** Existing `stage6_lifecycle_harness.py` or `test_stage6_*` checks exact step count of Scenario G.
  - **Mitigation:** Run full `pytest` locally and full `oracle-integration` on OCI before merging.

## Rollback or recovery

Revert the PR commit; no database schema or bank API changes are introduced.

## Open questions

None.
