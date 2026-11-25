# Implementation Plan

## Summary

Fix the four generator-side P0 reliability blockers identified in the maintainer review before running a multi-day 365-day Stage 6 backfill + 24-hour live qualification (bank rollover batching will follow in a separate focused `bank/` packet):

1. **Multi-day backfill → live cutover flexibility (`P0-1`):** Allow `run-live --resume --live-at <T>` to attach or update `live_at` on a run whose `backfill_finished` is `True` and `live_started` is still unset (provided `T >= backfill_end` and all other run inputs—`seed`, `config`, `first`, `days`, `roster_slots`—are identical), instead of bricking 365-day runs when backfill crosses KST midnight.
2. **End-of-live-window normal-wave grace (`P0-2`):** In `_due()` during `phase == "live"`, if `now >= live_end` and a wave consists solely of `normal_*` operations, stop dispatching remaining normal waves cleanly instead of raising `RecoveryError("live measurement window expired before the next operation")`. Non-normal scenario waves (`A..G`, `lookalike`) continue to raise `RecoveryError` if missed.
3. **Retryable HTTP `503` backoff & longer clock timeout (`P0-3`):** In `BankClient._call()`, add bounded exponential backoff (default up to 5 retries: `0.25s, 0.5s, 1.0s, 2.0s, 4.0s`) when `bank` returns retryable `503` (and `503 not in accept`), preventing transient `A0013` pool/lock-wait/rollover-barrier responses from stranding waves in `unconfirmed`. Also give `clock_set`, `clock_advance`, and `go_live` a `180.0s` per-request HTTP timeout (vs `30.0s` default) so heavy rollover days do not time out on the client.
4. **Crash-safe daily pruning of completed `normal_*` journal rows (`P0-4`):** After each backfill day advances in `_run_phases()`, idempotently roll up that finished day's terminal (`succeeded`/`rejected`) `normal_*` operations into `runs.archived_normal.<YYYY-MM-DD>` before deleting them from `truth.stage6_operations`, and merge `archived_normal` into `Stage6Journal.totals()`. This bounds `truth.stage6_operations` to a single day's normal volume instead of accumulating ~9.51M documents across 365 days.

## Files and components that change

- `generator/saltation_gen/client/bank.py`: add bounded `503` retry with exponential backoff in `_call()` (configurable `retry_503`, `retry_backoff`) and per-request `clock_timeout=180.0` on `clock_set`, `clock_advance`, and `go_live`.
- `generator/saltation_gen/truth/stage6.py`:
  - Allow `claim()` to accept an updated `inputs.run.live_at` when `progress.backfill_finished` is `True`, `progress.live_started` is unset, `live_at >= existing backfill_end`, and all non-`live_at` inputs match identically; persist the updated `inputs.run.live_at` atomically on the run document.
  - Add `prune_completed_normal_day(day_iso: str)` using idempotent `$set` of `archived_normal.<day_iso>` (only if not yet archived) followed by `delete_many` of that day's terminal `normal_*` rows, and update `totals()` to sum `archived_normal` + live `stage6_operations`.
- `generator/saltation_gen/drivers/stage6.py`:
  - In `_due()` (`phase == "live"`), skip/break cleanly when `now >= live_end` on purely `normal_*` waves while preserving `RecoveryError` for missed fraud/lookalike scenario waves.
  - Call `await journal.prune_completed_normal_day(completed_day.isoformat())` after `backfill_day` advances in `_run_phases()`.
  - Allow `run-live --resume --live-at <T>` to re-open a `backfill`-only run (`completed: True` with `summary.status == "completed"` and `backfill_finished: True` and no `live_started`) when transitioning into `phase == "live"`.
- `generator/tests/test_stage2.py`, `generator/tests/test_stage6_run.py`, `generator/tests/test_stage6_journal.py`: unit tests covering all four behaviors and edge cases.

## Order of work

1. Add `503` exponential-backoff retry and `clock_timeout` in `generator/saltation_gen/client/bank.py` + unit tests in `generator/tests/test_stage2.py`.
2. Add post-backfill `live_at` attachment in `Stage6Journal.claim()` and `prune_completed_normal_day()` + `totals()` merge in `generator/saltation_gen/truth/stage6.py` + unit tests in `generator/tests/test_stage6_journal.py` (including crash-between-archive-and-delete idempotency).
3. Update `generator/saltation_gen/drivers/stage6.py` for normal-wave `live_end` grace, daily pruning call, and `run-live --resume --live-at` after `run-backfill` + unit tests in `generator/tests/test_stage6_run.py`.
4. Run `repo.py status --change stage6-365d-resilience`, `pytest` + `ruff` on `generator/`, and `repo.py verify --change stage6-365d-resilience --group core` + `--group python`.

## Tests and proof

- `generator/tests/test_stage2.py`:
  - `BankClient._call` retries `503` up to `retry_503` times and succeeds when a later attempt returns `200`.
  - `BankClient._call` does NOT retry when `503` is in `accept`, nor on `400`/`500`.
  - `clock_set` / `clock_advance` / `go_live` pass the extended `clock_timeout`.
- `generator/tests/test_stage6_journal.py`:
  - `prune_completed_normal_day` archives terminal `normal_*` rows into `archived_normal`, deletes them from `stage6_operations`, preserves non-normal scenario rows, and produces identical `totals()` before and after pruning (including when called twice to simulate crash-before-delete recovery).
  - `claim()` accepts an updated `live_at` when `backfill_finished` is `True` and `live_started` is unset, while still rejecting changes to `seed`/`config`/`first`/`days` or `live_at` changes after `live_started` is set.
- `generator/tests/test_stage6_run.py`:
  - `_due` in `live` phase returns cleanly without raising when `now >= live_end` on a `normal_*` wave, while still raising `RecoveryError` for a missed scenario wave (`F`).
- All existing `generator` unit tests (`214+` tests), `ruff`, `control-plane`, and OCI `oracle-integration` (including `stage6_lifecycle_harness.py`) pass.

## Risks and mitigations

- **Risk:** Double-counting or losing `normal_*` totals if a crash occurs between archiving counts and deleting rows from `stage6_operations`.
  - **Mitigation:** Store per-day rollups under `archived_normal.<day_iso>` only when `archived_normal.<day_iso>` is absent; on `totals()`, only count remaining `stage6_operations` whose day is NOT in `archived_normal` (or filter out archived days via `episode_id` prefix check in `totals()`), guaranteeing exact idempotency even if a crash leaves undeleted rows behind.
- **Risk:** Retrying `503` on a request that somehow committed.
  - **Mitigation:** `bank` only returns `503` (`A0013`) before committing (pool acquire timeout, `FOR UPDATE WAIT 1`, or clock barrier busy), and every money operation carries an idempotent `X-Request-Id`.

## Rollback or recovery

Revert the PR commit; no database schema or bank API changes are introduced.

## Open questions

None.
