# Plan — customer-session-expiry

## Context

Astra Pro audit finding `F-03`: customer bearer tokens expire after 30 minutes of business-time inactivity (`SELECT_TOKEN` enforces `t.expires_at > :now`), but `AUTH_SESSION` is never closed on expiry (`ended_at` and `end_reason` remain `NULL`) and no `session.expired` business event is written to `OUTBOX` (unlike `ATM_SESSION`, which has `AtmSessionExpiryJob` + `atm.session_ended`).

## Implementation steps

1. **Apply approved `spec.md` baseline update**
   - Copy `changes/customer-session-expiry/spec.md` to root `spec.md` (updating `F-2` and `F-27` to specify customer session expiry materialization, `session.expired` event semantics, and the `auth_session_expiry` rollover job).

2. **Add sparse index migration for open customer sessions (`bank/migrations/0022_auth_session_expiry.sql` & `bank/src/shared/db/migrations.rs`)**
   - Create a function-based sparse index `auth_session_open_last_seen_ix ON auth_session (CASE WHEN ended_at IS NULL THEN last_seen END)` so day-rollover expiry scans only open sessions and never full-scans historical closed sessions.
   - Register `migration!(22, "0022_auth_session_expiry")` in `bank/src/shared/db/migrations.rs` so `db.migrate()` applies it.

3. **Harden token lookup and deferred touch against expiry races (`bank/src/shared/auth/`)**
   - Update `bank/src/shared/auth/sql/select_token.sql` to require `s.ended_at IS NULL` in addition to `t.revoked_at IS NULL AND t.expires_at > :now`.
   - Update `bank/src/shared/auth/sql/deferred_touch.sql` so an already-ended session (`ended_at IS NOT NULL`) cannot slide `access_token.expires_at` or `auth_session.last_seen` (lock `access_token` then `auth_session` with `FOR UPDATE WAIT 1`, check `ended_at IS NULL` before updating either row).
   - Add a lazy expiry path in `AuthCustomer::from_request_parts` (`bank/src/shared/auth/mod.rs`) with bounded SQL (`select_expired_token.sql` / `expire_session.sql`) using the exact same lock order (`access_token` then `auth_session` `FOR UPDATE WAIT 1`): when a presented customer bearer token is unrevoked, expired (`t.expires_at <= :now`), and its `auth_session` is still open (`ended_at IS NULL AND last_seen <= :cutoff`), conditionally close `auth_session` (`ended_at = last_seen + INTERVAL '30' MINUTE`, `end_reason = 'expired'`) and record `Event::for_request(&ctx, "session.expired", now, "MOBILE", "completed")` (with `customer_no`, `session_id`, `device_id`, and payload `{session_id, ended_at, end_reason: "expired"}`) in the same Oracle transaction before returning `401 Unauthorized`.

4. **Add `AuthSessionExpiryJob` rollover job (`bank/src/features/auth/` & `bank/src/main.rs`)**
   - Mirror `AtmSessionExpiryJob` (`bank/src/features/atm/mod.rs`): implement `AuthSessionExpiryJob` (`name() -> "auth_session_expiry"`) querying open sessions with `CASE WHEN s.ended_at IS NULL THEN s.last_seen END <= :cutoff` (`cutoff = at - SESSION_IDLE`) and processing in bounded batches of 200.
   - For each candidate, conditionally execute `UPDATE auth_session SET ended_at = last_seen + INTERVAL '30' MINUTE, end_reason = 'expired' WHERE session_id = :session_id AND ended_at IS NULL AND last_seen <= :cutoff`; if `1` row updated, write `Event::system("session.expired", at, "completed")` (with `customer_no`, `session_id`, `device_id`, and payload `{session_id, ended_at, end_reason: "expired"}`) via `outbox::record` in the same Oracle transaction before `s.commit()`.
   - Register `bank::features::auth::AuthSessionExpiryJob` in `bank/src/main.rs` alongside `AtmSessionExpiryJob`.

5. **Update event catalog, payload schema, and conformance tests**
   - Add `"session.expired"` to `contracts/codes/event-types.json` and update `"rollover.job_completed"` description to include `auth_session_expiry`.
   - Add `"session.expired"` payload schema (`{session_id: ULID, ended_at: date-time, end_reason: "expired"}`, `additionalProperties: false`) and `"auth_session_expiry"` to `rollover.job_completed.job` enum in `contracts/schemas/event-payloads.schema.json`.
   - Add representative positive and negative fixtures for `session.expired` (both system rollover and lazy request-triggered envelopes) in `scripts/ops/test_event_contracts.py`.

6. **Add integration and unit tests (`bank/tests/auth.rs`, `bank/tests/backfill.rs`, `bank/tests/migrations.rs`, `bank/src/shared/auth/mod.rs`)**
   - Cover lazy inactivity expiry on expired token (`401` + `ended_at` = effective expiry + `end_reason = 'expired'` + exactly one `session.expired` event).
   - Cover rollover expiry (`AuthSessionExpiryJob`) inside `backfill_sequence` before the live switch, idempotency on repeated sweep, and separation of `ended_at` from `occurred_at`.
   - Cover race safety: simultaneous logout/password-reset vs expiry, in-flight admitted touch preservation, and deferred touch not reviving an ended session.
   - Update `bank/tests/migrations.rs` to assert migrations `1..=22` and presence of `AUTH_SESSION_OPEN_LAST_SEEN_IX`.

## Verification

- Local Rust toolchain (`~/.cargo/bin`, `rustc 1.99.1`):
  - `cargo fmt --manifest-path bank/Cargo.toml --check`
  - `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`
  - `cargo test --manifest-path bank/Cargo.toml --locked --lib --bins`
  - `cargo test --manifest-path bank/Cargo.toml --locked --tests --no-run`
- Local Python/control-plane gates:
  - `python3 -m unittest scripts/ops/test_event_contracts.py -v`
  - `python3 scripts/ops/check_new_comments.py`
  - `python3 scripts/repo.py status --change customer-session-expiry --base event-payload-contracts`
- Deferred to PR CI after OCI 365-day backfill completes:
  - Full `repo.py verify --change customer-session-expiry --full` including Oracle-backed `bank/tests/auth.rs` and `bank/tests/backfill.rs`.
- Commit locally on branch `customer-session-expiry` (no remote push while OCI 365-day backfill is active).
