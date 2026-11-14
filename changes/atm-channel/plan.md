# Implementation Plan

## Summary

This plan delivers **stage 3** of the spec's Delivery table (ATM channel, API server) in one change: ATM terminals with coordinates, ATM PIN credentials, ATM sessions, and cash withdrawal, cash deposit and balance inquiry at a terminal. It is API server work only (no UI, by the owner's decision of 2026-11-12). The card channel (stage 4), the admin interface (stage 5), generator personas and scenarios (stage 6), the web UI (stage 7) and operations (stage 8) stay out.

Stages 1 and 2 left the bank with every mobile and inbound-deposit interface, the simulated clock, the day rollover and, with `request-outcomes`, one completion event for every request and stored rejected attempts. After this change the generator can seed terminals and PINs and drive cash through the ATM interface, which scenarios B, C and D (stage 6) and the ATM side of G need.

This change starts from `change/request-outcomes` (pull request #9), because the ATM routes use its outcome recording and rejected-attempt storage. When #9 is merged, this branch is rebased onto `main` and `base` in `change.json` is updated before the owner approves the plan.

### Spec acceptance criteria proved by this change

At test scale (integration tests, no 365-day backfill):

- **AC 4 (part):** for ATM activity, the records scenarios B, C and D require exist in Oracle (`ATM_SESSION`, `atm_cash` rows, `DAILY_ACCOUNT_TOTAL` cash in and out, rejected attempts) and the `atm.*` and `request.completed` events exist in `bank_logs`, each ATM event with the terminal's region and coordinates.
- **AC 6 (part):** daily close reports zero mismatches across dates with ATM cash; an `atm_cash` transfer that breaks F-21 is reported.
- **AC 9 (part):** in backfill mode ATM records take business time from the simulated clock; session expiry and the PIN lock are measured in business time.

Functional requirements covered: F-7 (the ATM PIN), F-16 (the ATM limits), F-17, F-18 and F-19 for ATM operations, F-20 to F-22 for `atm_cash`, F-23's rule for `atm_cash`, F-29 (ATM terminals), F-30 (the ATM interface on the generator listener), F-31 and F-35 for the new events. The spec's ATM interface, `ATM_CREDENTIAL`, `ATM_TERMINAL` and `ATM_SESSION` are built as the spec lists them.

### Decisions

- **One change for the whole stage,** cut into 8 units below, each with its own proof, committed one by one.
- **No UI work** (owner, 2026-11-12). No web app changes.
- **The PIN comes with seeding.** F-29 has the generator create credentials, so `POST /internal/v1/seed/accounts` gains `atm_pin` (exactly 4 digits), required for a demand account and refused for any other subtype. The bank stores only its Argon2id hash in `ATM_CREDENTIAL` (N-6, same parameters as passwords); the PIN appears in no response, log line or event. The generator keeps the PIN in its `truth` roster next to the password, as it does for passwords. Existing callers of the seeding interface (the generator and the test helpers) are updated in this change.
- **Terminals.** `POST /internal/v1/seed/atm_terminals` takes `terminal_id` (up to 20 characters, the `terminal_id` columns' width), `region` (an ISO 3166-2 code of Korea, `KR-11` to `KR-50`), `location` (up to 60 characters) and `latitude` and `longitude` as JSON strings of decimal degrees with at most 6 decimal places, inside Korea's bounding box (latitude 33.0 to 38.7, longitude 124.5 to 132.0). Coordinates are stored as `NUMBER(9,6)`.
- **Requests at a terminal.** On `/atm/v1/**` the terminal is the place: the request row's `terminal_id` and `region` are the terminal's, its IP and device are null, and `X-Sim-Ip`, `X-Sim-Region` and `X-Sim-Device` are ignored. `X-Request-Id` is used as on every generator request. The channel is `ATM`.
- **Session start** (`POST /atm/v1/session`: `terminal_id`, `account_no`, `pin`). Order of checks: unknown terminal (`404`, `A0003`); unknown account or one with no PIN (`A0004`, bank `404`), closed account (`A0004`, bank `403`); locked PIN (`A0004`, new bank code `409`, "PIN 입력 횟수 초과"); wrong PIN (`A0004`, new bank code `410`, "PIN 불일치"). Wrong PINs count per account; the third consecutive one locks ATM access until the start of the next business date (00:00 KST of D+1) and is itself answered `410`; a correct PIN resets the count. Unknown-account and wrong-PIN answers are indistinguishable in timing as far as the hash allows: an unknown account is checked against a fixed dummy hash.
- **A started session** writes one `ATM_SESSION` row (session ULID, terminal, account, started, request ID) and returns an opaque bearer token; only its SHA-256 hash is stored, as for customer tokens. A session lasts 3 minutes of business time from its start (F-17). A terminal serves one session at a time: starting a session ends the terminal's open session with reason `replaced`. `POST /atm/v1/session/end` ends it with reason `ended`; a request presenting an expired token ends it with reason `expired` and is answered `401`, `O0001`. `ATM_SESSION` gains `end_reason`, as `AUTH_SESSION` has one. A session that is never presented again stays open in the table and is expired by rule; no rollover job sweeps sessions, so F-27's job list does not change.
- **Rejected session attempts are stored** (F-19: wrong PIN, locked credential) as `TRANSACTION_REQUEST` rows with status `REJECTED`, type `ATM_SESSION`, channel `ATM`, the terminal, the response codes and null idempotency columns, as `request-outcomes` allows for rejected rows. Successful sessions are `ATM_SESSION` rows and need no request row.
- **Cash operations** (`POST /atm/v1/cash/withdraw`, `POST /atm/v1/cash/deposit`: `idempotency_key` up to 40 characters, `amount` as a JSON string of an integer from 10,000 to 1,000,000,000 KRW in multiples of 10,000, since an ATM takes and gives notes). The idempotency scope is the terminal (F-18): `ATM:<terminal_id>`. The request hash covers the operation, the session's account and the amount. They follow the transfer's pattern for replay, idempotency conflict (`409`, `A0007`, stored as a rejected attempt with null key) and concurrent duplicates.
- **Limits** (F-16): withdrawal at most 1,000,000 KRW per transaction and 6,000,000 KRW per account per business date, summed in `DAILY_ACCOUNT_TOTAL.cash_out`; deposits have no limit and add to `cash_in`. Both withdrawal limits are configurable like the transfer limit (`SALTATION_ATM_WITHDRAW_LIMIT`, `SALTATION_ATM_WITHDRAW_DAILY_LIMIT`, defaults as above). Exceeding one is `A0004`, bank `406`; insufficient balance is bank `405`; a closed or non-demand account bank `403`. Every business rejection keeps its row with the key, as transfers do.
- **Ledger.** `Kind::AtmCash` (`atm_cash`, already allowed by the `TRANSFER.kind` check) writes one row on the session's account, negative for a withdrawal and positive for a deposit, channel `ATM`, the terminal ID, no counterparty, and a `TRANSFER` of kind `atm_cash`. F-21's check for `atm_cash` is one row whose amount is plus or minus the transfer's amount; daily close gains the same rule. The operation is customer-initiated (the session holder authenticated with the PIN), so it reactivates a dormant account like the owner's transfer.
- **Balance inquiry** (`GET /atm/v1/balance`) returns the session account's balance and writes an `atm.operation` event with the completion event in one transaction.
- **Events** (F-35), new types in `contracts/codes/event-types.json`, each carrying the terminal ID, region, latitude and longitude, the account and the request ID: `atm.session_started`, `atm.session_rejected` (with the bank response code, and whether this attempt locked the account), `atm.session_ended` (reason `ended`, `replaced` or `expired`), `atm.operation` (operation `withdraw`, `deposit` or `balance`; outcome `approved` or `rejected`; amount and response codes). Each is written in the transaction of the state it describes; `request.completed` follows as `request-outcomes` defines.
- **Outcome recording.** The ATM cash routes join the money-moving route table of `shared/outcome`, so a `401` on them is stored as a rejected attempt with type `ATM_WITHDRAW` or `ATM_DEPOSIT` and channel `ATM`.
- **The generator only gains what the tests need:** client methods for the ATM interface, seeding of terminals (a fixed list in configuration, one per region, coordinates of real city centres) and of PINs. Drivers for ATM behaviour are stage 6.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `contracts/openapi/atm.yaml` | new: the ATM interface |
| `contracts/openapi/internal.yaml` | `seed/atm_terminals`; `atm_pin` on `seed/accounts` |
| `contracts/codes/response-codes.json`, `contracts/codes/event-types.json`, `contracts/schemas/log-envelope.schema.json` | bank codes `409` and `410`; the `atm.*` events and their payload fields |
| `bank/migrations/0016_atm.sql`, `bank/src/shared/db/migrations.rs` | `ATM_TERMINAL`, `ATM_CREDENTIAL`, `ATM_SESSION` (with `end_reason`, token hash, one open session per terminal) |
| `bank/src/features/atm/` | new: router, session, cash, balance, `sql/` |
| `bank/src/features/internal/` | seed terminals; PIN on seeded accounts |
| `bank/src/features/daily_close/` | the `atm_cash` rule |
| `bank/src/shared/ledger/` | `Kind::AtmCash`, terminal ID on rows, cash daily totals |
| `bank/src/shared/limits.rs` | the two withdrawal limits |
| `bank/src/shared/context/`, `bank/src/shared/outcome/` | ATM request context (terminal as place, headers ignored); ATM routes in the outcome table |
| `bank/src/app.rs`, `bank/src/main.rs` | ATM routes on the generator listener only |
| `bank/tests/` | new suite `atm`; seeding helpers send PINs; `listeners` gains the ATM routes; `daily_close` cases in `backfill` |
| `generator/saltation_gen/`, `generator/tests/` | ATM client, terminal list, PIN seeding and roster field |
| `scripts/dev/it-oracle.sh` | target `atm`, in `all` with the ordinary suites |
| `infra/compose/compose.yml`, `infra/compose/.env.example` | the limit variables, if Compose has to pass them |
| `docs/dev-host.md` | the new target |

## Order of work

Each unit names its proof, run from the repository root. Commit after each unit.

1. **Contracts and schema.** The contract files above; migration 16.
   - Proof: `scripts/dev/it-oracle.sh migrations`; `python3 scripts/repo.py verify --change atm-channel --group web` (the mobile app is unaffected).
2. **Seeding.** Terminals; PINs on demand accounts; test helpers and generator seeding send PINs.
   - Proof: `scripts/dev/it-oracle.sh atm` (seeding cases: validation of region, coordinates and PIN; a PIN refused on a term account; no PIN or hash in the response, log or outbox) and every ordinary suite still passing.
3. **Sessions.** Start, end, replacement, expiry, wrong PIN, lock until the next business date, rejected attempts stored.
   - Proof: `atm` (in live mode) and `backfill` (after a clock advance of 3 minutes the token is refused and the session ended `expired`; after three wrong PINs the account is locked and a correct PIN is refused until the clock crosses midnight, then accepted).
4. **Ledger and cash.** `atm_cash`, withdraw and deposit, limits, idempotency, dormant reactivation.
   - Proof: `atm`: one row and one transfer per operation with the terminal ID; balance and daily totals; both limits and their configuration; insufficient balance; replay, conflict and eight concurrent duplicates (one movement); a dormant account reactivated by a withdrawal; `transfers` still passing.
5. **Balance inquiry and events.** `atm.operation` for every operation; all `atm.*` events with the terminal's region and coordinates, validated against the envelope schema.
   - Proof: `atm`: one event of the right type per session step and operation, with coordinates equal to the seeded terminal's; `outcomes` still passing with the ATM routes in its coverage table.
6. **Daily close.** The `atm_cash` rule.
   - Proof: `backfill`: dates with ATM cash close with zero mismatches; a tampered `atm_cash` row is reported.
7. **Generator.** Client methods, terminal list, PIN seeding and roster field.
   - Proof: `python3 scripts/repo.py verify --change atm-channel --group python`.
8. **Whole run.** `all` includes `atm`; `e2e` seeds terminals and runs one ATM withdrawal and deposit through the generator client after the normal run; docs.
   - Proof: `scripts/dev/it-oracle.sh all` on the OCI host, `scripts/dev/it-oracle.sh e2e`, then `python3 scripts/repo.py verify --change atm-channel --full`.

Implementation is delegated as in `request-outcomes`: the lead agent works on the owner's Mac (edits, commits, cargo checks and the database suites against local Oracle and MongoDB containers), runs the whole-stack `e2e` and `verify --full` on the OCI host, writes one closed brief per unit, and reruns each proof before committing. Coding workers run as `codex exec --yolo -m gpt-6.1-sol -c model_reasoning_effort=low`, stay inside `write_scope` and never commit. Units run one at a time.

## Tests and proof

The unit proofs above; every event written by the `atm` suite validates against the envelope schema; the suite's secret scan covers the PINs it uses; `repo.py verify --full`; the CI `summary` check on the pull request. No workflow file and no check registration changes.

## Risks and mitigations

- **PIN guessing.** Three attempts, then locked until the next business date; the lock is checked before the hash, and the dummy hash keeps unknown accounts from answering faster.
- **Argon2id cost at seeding.** Seeding 20,000 accounts hashes 20,000 PINs; the parameters are the passwords' (N-6), and the seeding interface already hashes passwords in batches. Stage 8 measures full-scale seeding.
- **Session state in backfill.** Expiry is computed from business time only; the `backfill` suite proves it across an advance.
- **One open session per terminal** is enforced by a unique function-based index on the terminal for rows without an end, so two concurrent starts at one terminal cannot both stay open.
- **CI time.** One more suite in `all`; if the `oracle` job's budget is exceeded, the pull request says by how much.

## Rollback or recovery

Before merge, close the pull request. After merge, revert the squash commit; migration 16 only adds objects, so a database that ran it needs `infra/oracle/reset_schema.sql` (test data only) to go back.

## Open questions

- None.
