# Plan — event-payload-contracts

## Context

Astra Pro audit finding `D-02`: `contracts/schemas/log-envelope.schema.json` validates `payload` only as a generic JSON object with a prose description, and `contracts/codes/event-types.json` documents the 20 business event types without executable payload schemas or a mechanical producer-to-catalog inventory check.

Note: `bank/tests/common/mod.rs` compiles `contracts/schemas/log-envelope.schema.json` standalone via `jsonschema::draft202012::new(&schema)` without external `$ref` resolution, so `log-envelope.schema.json` must remain self-contained while per-event payload contracts live in a dedicated schema file.

## Implementation steps

1. **Add executable per-event payload schemas (`contracts/schemas/event-payloads.schema.json`)**
   - Define a Draft 2020-12 JSON Schema (`$defs`) with an explicit payload schema for every one of the 20 event types in `contracts/codes/event-types.json`:
     `request.completed`, `login.succeeded`, `login.failed`, `analyst.login.succeeded`, `analyst.login.failed`, `logout`, `device.new`, `password_reset.requested`, `password_reset.completed`, `password_reset.failed`, `payee.registered`, `payee.deleted`, `account.status_changed`, `rollover.job_completed`, `atm.session_started`, `atm.session_rejected`, `atm.session_ended`, `atm.operation`, `card.authorized`, `card.declined`.
   - Match the exact wire shapes currently emitted by `bank/src/**/*.rs` (descriptive first: zero wire-format breaking changes).
   - Keep `contracts/schemas/log-envelope.schema.json` self-contained and compatible with `bank/tests/common/mod.rs` (only update prose description if needed to reference `event-payloads.schema.json`).

2. **Add static producer-inventory & schema-conformance test suite (`scripts/ops/test_event_contracts.py`)**
   - Pure Python stdlib `unittest` (no new dependencies, zero comments/docstrings per `scripts-comment-free`).
   - Verify 1:1 coverage across three sources:
     1. Every `event_type` key in `contracts/codes/event-types.json`
     2. Every payload schema entry in `contracts/schemas/event-payloads.schema.json`
     3. Every business event emitted by `bank/src/**/*.rs` (`Event::for_request`, `Event::system`, ATM `event(...)`, admin/password_reset/card dynamic event-type literals)
   - Validate representative emitted event fixtures (all 20 event types, including money-moving vs non-money-moving `request.completed` and optional/nullable fields) against both `log-envelope.schema.json` and `event-payloads.schema.json`.
   - Include negative fixture tests proving that missing required payload fields, wrong field types, unknown extra payload fields, unknown `event_type`, or an uncataloged producer literal fail verification.

3. **Register the conformance check in `checks.json`**
   - Add `ops-event-contracts` (`group: "python"`, `argv: ["python3", "-m", "unittest", "scripts/ops/test_event_contracts.py", "-v"]`, `requires: ["python3"]`) routed on `contracts/**`, `bank/src/**`, `scripts/ops/test_event_contracts.py`, and `checks.json`.

## Verification

- Run `python3 -m unittest scripts/ops/test_event_contracts.py -v`.
- Run `python3 scripts/ops/check_new_comments.py`.
- Run `python3 scripts/tests/run.py` (control-plane tests including `checks.json` validation).
- Run `python3 scripts/repo.py status --change event-payload-contracts --base generator-contract-registry`.
- Commit locally on branch `event-payload-contracts` (no push while OCI 365-day backfill is active).
