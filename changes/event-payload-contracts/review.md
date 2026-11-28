# Final Review — event-payload-contracts

- **Reviewed implementation commit:** `be1888ac3c8c046b530170e7daed0d6d26aadd1a` (base `add9a97` + review-fix commit `be1888a`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Neon (`gemini-4-neon-high`, read-only)

## Verification evidence

- `python3 -m unittest scripts/ops/test_event_contracts.py -v`: 24 tests in 0.293s (`OK`).
- `python3 scripts/ops/check_new_comments.py`: exit 0, no findings (zero comments and zero docstrings in `scripts/ops/test_event_contracts.py`).
- `python3 scripts/tests/run.py`: 317 run, 4 skipped, 0 failures (`OK`).
- `python3 scripts/repo.py status --change event-payload-contracts --base generator-contract-registry`: `status: ok`, `readiness=ready`, 6 changed paths within `write_scope`.
- Confirmed `contracts/schemas/log-envelope.schema.json` remained unchanged and self-contained for `bank/tests/common/mod.rs`.
- **Deferred pre-merge gate:** Docker-dependent `python3 scripts/repo.py verify --change event-payload-contracts --full` is intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is active and must pass on the final PR head before merge.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Astra 6.2 | Important | Producer scanner did not exclude `#[cfg(test)]` items and did not resolve `Event` import aliases (`use ...::Event as Audit;`). | **Fixed** in `be1888a`: skips `#[cfg(test)]` items/constructor args, tracks `Event` and ATM helper import aliases, plus regression tests. |
| Astra 6.2 | Important | `request.completed`, `card.*`, `atm.*`, `login.*`, `account.status_changed`, `rollover.job_completed`, and `payee.*` schemas accepted values outside real Rust producer contracts (e.g. `failed_count: -1`, `business_date: "2026-02-31"`, `from == to`, `amount: "0000"`, `request_id: null` with non-`expired` ATM end reason, lone `counterparty_account_no`). | **Fixed** in `be1888a`: tightened all affected `$defs` against exact Rust producers (preserving valid edge cases such as leading-zero card amounts, request-triggered ATM `expired`, and lock-rejection null ATM cash amounts), added calendar `format: "date"` and `minLength` validator support, and added 20 negative fixture assertions. |
| Astra 6.2 | Important | Static conformance suite cannot detect a payload key rename at an existing Rust producer without runtime Rust validation. | **Deferred follow-up**: `bank/tests/**` is outside this packet's `write_scope` (no local Rust toolchain yet); wiring `event-payloads.schema.json` into `bank/tests/common/mod.rs` is tracked as the next Rust-side follow-up. |
| Astra 6.2 | Important | Local `repo.py verify --full` evidence is not yet recorded for this revision. | **Deferred to PR CI**: local Mac has no Docker daemon and OCI runner is reserved for the active 365-day backfill; full `repo.py verify --full` will run before merge. |
| Neon | Minor | Deferred-init `let kind;` shadowing in `remember_binding` did not invalidate an outer binding; negative fixtures for `card.*` `oneOf` invariants recommended. | **Fixed** in `be1888a`: deferred-init shadowing now fails closed with a regression test, and `card.*` negative fixtures added. |

## Conclusion

All in-scope Important findings and Minor suggestions from both independent reviewers are resolved and verified locally (`24 passed` in `test_event_contracts.py`, `317 passed` in control-plane tests). Ready for final PR-head `repo.py verify --full` once the active OCI 365-day backfill completes.
