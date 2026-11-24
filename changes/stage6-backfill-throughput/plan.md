# Plan: Batch Stage 6 Backfill Journaling, Clock Checks, and Outbox Relay Delivery

## Goal

Eliminate the `generator` + `mongo` Stage 6 backfill serialization bottleneck identified in `docs/stage8-oci-sizing-report.md` (currently ~19 MongoDB round-trips and ~5.1 synchronous `j=True` writes per business operation behind a single `asyncio.Lock`, capping backfill at ~46 business ops/s while `bank` is idle), plus fix companion MongoDB/`outbox_relay` bottlenecks (`outbox_relay` micro-batch `j=True` thrashing, missing `bank_logs.events` `recorded_at` index, `card_networks` per-op lookup, redundant `GET /clock` calls, and `totals()` full-collection scan), without changing any product spec requirements or weakening F-34 pre-send label durability, crash recovery, or simulated clock invariants.

## Scope

- `generator/saltation_gen/truth/stage6.py`
- `generator/saltation_gen/truth/recovery.py`
- `generator/saltation_gen/drivers/backfill_clock.py`
- `generator/saltation_gen/drivers/stage6_execute.py`
- `generator/saltation_gen/drivers/stage6.py`
- `generator/tests/test_stage6_journal.py`
- `generator/tests/test_stage6_run.py`
- `bank/src/features/outbox_relay/mod.rs`

## Order of work

1. **Batch journal primitives & `$group` totals (`generator/saltation_gen/truth/stage6.py`, `generator/saltation_gen/truth/recovery.py`)**
   - Keep all existing single-operation `Stage6Journal` and `RecoveryJournal` methods intact for scenario steps, live mode, and existing tests.
   - Add `Stage6Journal.prepare_batch(records)` (single lease check + `insert_many(..., ordered=False)` with `WriteConcern(w=1, j=True)`, falling back to `_id: {$in: ...}` lookup & `_stable_shape` verification only on `BulkWriteError` code `11000` during resume).
   - Add `RecoveryJournal.ensure_labels_batch(labels_by_rid)` (`insert_many(..., ordered=False)` with `WriteConcern(w=1, j=True)`, verifying via `$in` readback on `BulkWriteError` `11000` so F-34 pre-send durability and conflict detection are strictly preserved).
   - Add `Stage6Journal.mark_unconfirmed_wave(entries)` (single lease check + fenced `update_many` or `bulk_write` moving `prepared -> unconfirmed` with `j=True`, asserting every distinct row matched before any HTTP send) and `Stage6Journal.complete_wave(outcomes)` (fenced `bulk_write(..., ordered=False)` with `j=True` committing all confirmed `succeeded`/`declined` results before raising on any failed sibling).
   - Replace `Stage6Journal.totals()`'s Python cursor scan over all operation documents with a MongoDB `$group` aggregation over `state`.
   - Proof: `pytest generator/tests/test_stage6_journal.py` (with new unit tests covering `prepare_batch`, `ensure_labels_batch`, `mark_unconfirmed_wave`, `complete_wave`, partial-failure flush, lease loss, and `$group` `totals()`).

2. **Clock & executor round-trip elimination (`generator/saltation_gen/drivers/backfill_clock.py`, `generator/saltation_gen/drivers/stage6_execute.py`)**
   - In `BackfillClock`, add a Stage 6 backfill clock advance helper that validates the `ClockState` returned directly by `POST /internal/v1/clock/set` (eliminating the redundant post-set `GET /internal/v1/clock`) and skips redundant `GET /internal/v1/clock` calls between equal-timestamp waves while retaining durable `current`/`intended` updates on distinct advances and midnight rollovers. Keep existing `move_clock()` behavior intact for `cards.py` / `behavior.py`.
   - In `Stage6Executor`, cache `truth.card_networks` credentials in memory per run (eliminating ~2.15M redundant `find_one` calls/yr), cache `home_context` per customer, skip redundant `_cash_time()` `GET /clock` calls when `backfill=True` (lease holder exclusively owns `SIM_CLOCK`), and add `execute_wave(items, now, prepared_map)` that performs lock-free concurrent HTTP via `asyncio.gather` between `mark_unconfirmed_wave` and `complete_wave`.
   - Keep scenario steps (`not episode.kind.startswith("normal_")`), recovery logins (`_refresh`), and response-dependent steps (`term_identity`/`term_close`) on the existing scalar path.
   - Proof: `pytest generator/tests/test_stage6_run.py generator/tests/test_stage6_lifecycle_pacing.py generator/tests/test_stage6_deposit_identity.py`.

3. **Chunked backfill admission & checkpointing (`generator/saltation_gen/drivers/stage6.py`)**
   - Within each 60-second backfill bucket, group consecutive normal-only waves into bounded chunks (up to 256 operations, ending on wave boundaries) so a dense 1-minute bucket (up to 3,501 ops) still checks F-36a backpressure (`_gate`) well inside the 50,000-event pause watermark.
   - Pre-prepare and pre-label each normal chunk via `prepare_batch` + `ensure_labels_batch`, then dispatch each wave via `execute_wave`.
   - Checkpoint `runs.progress` (`wave_date`, `wave_offset`, `current`) at chunk boundaries, midnight rollovers, and graceful `stop.is_set()` exits; on resume, skip already-terminal waves before requesting a clock move so backward clock moves cannot occur.
   - Proof: `pytest generator/tests/test_stage6_*.py` (all 7 Stage 6 test modules green, plus new regression tests for chunked stop/resume across multiple instants in one bucket and partial-wave failure).

4. **`outbox_relay` partial-batch linger & `recorded_at` index (`bank/src/features/outbox_relay/mod.rs`)**
   - Add `PARTIAL_BATCH_LINGER = Duration::from_millis(35)` in `deliver()`: sleep `35 ms` when `0 < delivered < BATCH_SIZE` so steady traffic coalesces into larger `insert_many(j=True)` batches instead of spinning on 2–4 event flushes, while full `BATCH_SIZE` (`500`) batches loop immediately with zero sleep.
   - Ensure `{recorded_at: 1}` index is created on `bank_logs.events` in `events_collection()` so N-9 18-month retention deletes use an index instead of a full collection scan.
   - Proof: `cargo test --manifest-path bank/Cargo.toml --lib` and `cargo clippy --manifest-path bank/Cargo.toml --all-targets -- -D warnings`.

5. **Full verification & OCI smoke check**
   - Run `python3 scripts/repo.py status --change stage6-backfill-throughput` and `python3 scripts/repo.py verify --change stage6-backfill-throughput --full` (local checks + CI full matrix).
   - Review diff against `REVIEW.md` (including OTN clause: no Oracle timings in committed files or PR body).

## Risks and rollback

- **Risk 1 (F-34 pre-send label or crash-recovery regression):** Batching could accidentally send HTTP before labels are durable or leave confirmed siblings `unconfirmed` when one item in a wave fails.
  - **Mitigation:** Strictly enforce `prepare_batch(j=True)` → `ensure_labels_batch(j=True)` → `mark_unconfirmed_wave(j=True, matched == W)` → HTTP → `complete_wave(j=True)` before raising; verify with `test_stage6_lifecycle_pacing.py` (`ProofTransport` asserts `unconfirmed` + committed labels at HTTP entry) and new fault-injection tests.
- **Risk 2 (Resume backward-clock error inside a partially completed chunk):** If a process crashes mid-chunk after advancing the clock, re-scanning from the chunk start could attempt a backward clock move.
  - **Mitigation:** Skip waves whose operations are already terminal *before* calling clock advance on resume, and add a unit test covering mid-chunk multi-instant crash & resume.
- **Risk 3 (`outbox_relay` lag):** A 35 ms linger on partial batches adds at most 35 ms latency (well below the 30s F-36a alert threshold) and zero delay when a full 500-event batch is available.
- **Rollback:** Revert the single PR commit; no database schema or wire-contract changes are introduced (`{recorded_at: 1}` index is additive and backwards-compatible).
