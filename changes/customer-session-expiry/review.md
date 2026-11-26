# Final Review — customer-session-expiry

- **Reviewed implementation commit:** `d0eecceb517ecf35479c425919358a296ad1d350` (base `ee10bb0` + review-fix commit `d0eecce`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Neon (`gemini-4-neon-high`, read-only)

## Verification evidence

- Local Rust toolchain (`rustc 1.99.1`, `cargo 1.99.1`):
  - `cargo fmt --manifest-path bank/Cargo.toml --check`: passed.
  - `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`: passed (0 warnings).
  - `cargo test --manifest-path bank/Cargo.toml --locked --lib --bins`: 80 passed, 0 failed.
  - `cargo test --manifest-path bank/Cargo.toml --locked --tests --no-run`: all integration test binaries (`auth.rs`, `backfill.rs`, `migrations.rs`, etc.) compiled cleanly.
- Local Python / contract checks:
  - `python3 -m unittest scripts/ops/test_event_contracts.py -v`: 24 passed (`OK`), covering all 21 business event types including `session.expired` and rollover job `auth_session_expiry`.
  - `python3 scripts/ops/check_new_comments.py`: exit 0, no findings.
- **Deferred pre-merge gates:**
  - Oracle-backed `repo.py verify --change customer-session-expiry --full` (including live execution of `bank/tests/auth.rs`, `bank/tests/backfill.rs`, `bank/tests/migrations.rs`, and `AUTH_SESSION_OPEN_LAST_SEEN_IX` execution-plan verification) is intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is active and must pass on the final PR head before merge.
  - Stacked-branch `stale-baseline` against unmerged `main:spec.md` resolves automatically once earlier stacked PR `document-loopback-trust-topology` merges to `main`.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Neon | Critical | `bank/tests/migrations.rs:30` (and `populated_admin_upgrade`) asserted migrations `1..=21`, which would fail in CI after adding migration `22`. | **Fixed** in `d0eecce`: added `bank/tests/migrations.rs` to `write_scope`, re-approved `plan.md`, and updated assertions to `1..=22` (and `2` / `22` for `populated_admin_upgrade`). |
| Astra 6.2 + Neon | Critical / Important | Standalone `#[tokio::test]` in `bank/tests/backfill.rs` calling `clock.set(start)` would fail with `ClockError::NotBackfill` because `backfill_sequence` permanently switches `sim_clock` to `LIVE` on the shared test schema. | **Fixed** in `d0eecce`: moved into `async fn auth_session_expiry_rollover(api: &Api)` invoked inside `backfill_sequence` before `live_switch_is_one_way`, registered `AuthSessionExpiryJob` in `backfill_sequence`'s `Registry`, and used `clock.advance(...)`. |
| Astra 6.2 | Important | Lazy expiry could close a session while an earlier-admitted request still held an uncommitted `PendingTouch`, discarding its valid pre-deadline touch. | **Fixed** in `d0eecce`: added process-local RAII `InFlightSessionGuard` on `PendingTouch` so `expire_presented_token` skips closing any session with an admitted in-flight request, with unit test `in_flight_session_guard_tracks_active_requests` and updated integration test in `bank/tests/auth.rs`. |
| Astra 6.2 + Neon | Important | Concurrent `logout` racing with lazy expiry ignored `END_SESSION` affected rows and unconditionally emitted a completed `logout` event. | **Fixed** in `d0eecce`: `logout` checks `s.execute(&END_SESSION, ...)? == 1`; if `0`, rolls back without emitting `logout` and returns `401 Unauthorized`. |
| Neon + Astra 6.2 | Important / Minor | Lock-contention error mapping handled `DbError::Busy` and `ORA-30006`, omitting `ORA-00054` and `ORA-00060`. | **Fixed** in `d0eecce`: shared `is_lock_contention()` classifier mapping `DbError::Busy` and `ORA-00054 | ORA-00060 | ORA-30006` consistently to `503 Busy` in both lazy expiry and `PendingTouch::apply`. |
| Neon | Minor | `AuthSessionExpiryJob` discarded `last_seen` and ran an extra per-row `SELECT_ENDED_AT`; `deferred_touch.sql` ran two queries on `auth_session` and lacked `revoked_at` guard; missing `common::envelope_errors(&event)` assertions. | **Fixed** in `d0eecce`: added `expire_session_with_ended_at` using `last_seen + SESSION_IDLE` directly, combined `deferred_touch.sql` `SELECT`s with `v_revoked_at IS NULL AND v_ended_at IS NULL`, and added `common::envelope_errors(event)` checks in `auth.rs` and `backfill.rs`. |
| Astra 6.2 | Important | Oracle execution-plan proof for `AUTH_SESSION_OPEN_LAST_SEEN_IX` and full `repo.py verify --full` evidence require live Oracle. | **Deferred to PR CI** after OCI 365-day backfill completes. |

## Conclusion

All Critical, Important, and Minor code findings from both independent reviewers are resolved and verified locally (`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `80` Rust unit tests, all integration test targets compiled, `24` event contract tests, `check_new_comments.py`). Ready for full Oracle-backed `repo.py verify --full` on PR CI once the active OCI 365-day backfill completes.
