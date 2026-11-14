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
- **The PIN comes with seeding.** F-29 has the generator create credentials, so `POST /internal/v1/seed/accounts` (which opens demand accounts only) gains `atm_pin`, exactly 4 digits, required. The bank stores only its Argon2id hash in `ATM_CREDENTIAL` (N-6, the passwords' parameters); the PIN appears in no response, log line or event. The generator keeps the PIN in its `truth` roster next to the password. Term and loan accounts, opened through `/v2.0/ext/**`, get no PIN and cannot start an ATM session. Existing callers of the seeding interface (the generator and the test helpers) are updated in this change. **No upgrade path:** accounts seeded before this change have no PIN and are refused at the ATM like an unknown account; the integration targets reset the schema, and a long-lived database (the local stack, the OCI `e2e` database) is reset before ATM use. The `e2e` target seeds its own customers, so it needs no reset.
- **Terminals.** `POST /internal/v1/seed/atm_terminals` takes `terminal_id` (up to 20 characters, the existing `terminal_id` columns' width), `region` (an ISO 3166-2 code of Korea, `KR-11` to `KR-50`), `location` (up to 60 characters) and `latitude` and `longitude` as JSON strings of decimal degrees with at most 6 decimal places, inside Korea's bounding box (latitude 33.0 to 38.7, longitude 124.5 to 132.0). Coordinates are stored as `NUMBER(9,6)`.
- **Simulated context is kept** (F-31): ATM requests carry `X-Request-Id`, `X-Sim-Ip`, `X-Sim-Region` and `X-Sim-Device` like every generator request, and the request row stores them. The place of the operation is the terminal: the request row also stores `terminal_id`, and every `atm.*` event carries the terminal's region, latitude and longitude from `ATM_TERMINAL`. The generator sends the terminal's region as `X-Sim-Region`; the bank does not enforce that. The channel is `ATM`.
- **Session start** (`POST /atm/v1/session`: `terminal_id`, `account_no`, `pin`). The PIN is always hashed first, against the account's hash or, when there is none, a fixed dummy hash, so every outcome takes one Argon2id verification. Then, in one transaction that locks the terminal row and the account's `ATM_CREDENTIAL` row (`SELECT ... FOR UPDATE`, terminal first), the bank rereads the credential and decides, in this order: unknown terminal (`404`, `A0003`); unknown account or one without a PIN (`422`, `A0004`, bank `404`); closed account (`422`, `A0004`, bank `403`); locked (`locked_until` after now: `422`, `A0004`, new bank code `409` "PIN 입력 횟수 초과"); wrong PIN (`422`, `A0004`, new bank code `410` "PIN 불일치"). A lock whose time has passed is cleared first, with the failure count back to 0, so the next wrong PIN counts as the first. The third consecutive wrong PIN sets `locked_until` to the start of the next business date (00:00 KST of D+1) and is answered `410`; a correct PIN sets the count to 0. Locking the credential row makes concurrent attempts on one account count one by one.
- **A started session** writes one `ATM_SESSION` row (session ULID, terminal, account, started, request ID) and returns an opaque bearer token with its expiry; only the token's SHA-256 hash is stored, as for customer tokens. A session lasts 3 minutes of business time from its start (F-17). The response schema of every ATM route is in `contracts/openapi/atm.yaml`: session start returns `atm_session_token` and `expires_at`; withdrawal and deposit return the amount, the balance after, the account sequence and `request_id`; balance returns the balance.
- **Session end and expiry.** `ATM_SESSION` gains `end_reason` (`ended`, `replaced`, `expired`, `locked`), as `AUTH_SESSION` has one. A terminal serves one session at a time; the locked terminal row serializes starts at it. When a session starts, the terminal's open session is ended: as `expired` with `ended_at` = its start + 3 minutes if that has passed, otherwise as `replaced` at now. `POST /atm/v1/session/end` ends it as `ended`. A request presenting a token whose session has passed its expiry and has no end is answered `401`, `O0001`, and in one transaction the handler sets `expired`, writes `atm.session_ended`, stores the rejected attempt (for the cash routes) and writes `request.completed`; a token whose session already ended is answered `401` with no state change. A session that ends because another request found it has `atm.session_ended` written with a null request ID and the session's own start request ID in the payload, so it is never labelled with someone else's request. Every transition updates only a row whose end is still null, so each session gets exactly one `atm.session_ended`. A session never presented again and never replaced stays open in the table, past its expiry by rule; see Open questions.
- **Every ATM operation rechecks the credential.** Withdrawal, deposit and balance inquiry are refused (`422`, `A0004`, bank `409`) while the account is locked, so three wrong PINs at one terminal stop a session open at another; the third wrong PIN also ends the account's open sessions as `locked`.
- **Rejected attempts are stored** (F-19). Every refused session start (unknown terminal, unknown or PIN-less account, closed account, locked, wrong PIN) is a `TRANSACTION_REQUEST` row with status `REJECTED`, type `ATM_SESSION`, channel `ATM`, the terminal ID when the terminal exists, the response codes and null idempotency columns, as `request-outcomes` allows for rejected rows; successful sessions are `ATM_SESSION` rows and need no request row. `atm.session_rejected` is written only when the terminal exists. Cash rejections follow the transfer's rules: business rejections keep the row with its key; a `401` (missing, unknown or expired token) is stored by the outcome table with the terminal ID when the token resolved to a session and null otherwise. `shared/outcome`'s response metadata and `insert_rejected.sql` gain the terminal ID for this.
- **Cash operations** (`POST /atm/v1/cash/withdraw`, `POST /atm/v1/cash/deposit`: `idempotency_key`, up to 20 characters so that a conflict can store it in `bank_tran_id` as `request-outcomes` does; `amount` as a JSON string of an integer from 1 to 999,999,999,999 KRW, the spec's amount domain). The idempotency scope is the terminal (F-18): `ATM:<terminal_id>`. The request hash covers the operation, the session's account and the amount. Replay, idempotency conflict (`409`, `A0007`) and concurrent duplicates follow the transfer.
- **Limits** (F-16): withdrawal at most 1,000,000 KRW per transaction and 6,000,000 KRW per account per business date, summed in `DAILY_ACCOUNT_TOTAL.cash_out`; deposits have no limit and add to `cash_in`. Both withdrawal limits are configurable like the transfer limit (`SALTATION_ATM_WITHDRAW_LIMIT`, `SALTATION_ATM_WITHDRAW_DAILY_LIMIT`, defaults as above). Exceeding one is `422`, `A0004`, bank `406`; insufficient balance bank `405`; a closed account bank `403`. The operation locks the account row first, then its daily total, so withdrawals with different keys at two terminals cannot both spend the last balance or the last of the daily limit.
- **Ledger.** `Kind::AtmCash` (`atm_cash`, already allowed by the `TRANSFER.kind` check) writes one row on the session's account: a withdrawal `inout_type` 출금, a deposit 입금, `tran_type` 현금, `print_content` `ATM출금` or `ATM입금`, channel `ATM`, the terminal ID, no counterparty; and a `TRANSFER` of kind `atm_cash`. F-21's check for `atm_cash` is one row whose amount is plus or minus the transfer's amount; daily close gains the same rule. A withdrawal or deposit is customer-initiated (the session holder authenticated with the PIN), so it reactivates a dormant account like the owner's transfer; a balance inquiry changes neither the account's status nor its last activity, like the mobile balance inquiry.
- **Balance inquiry** (`GET /atm/v1/balance`) returns the session account's balance and writes `atm.operation` with `request.completed` in one transaction.
- **Events** (F-35), new types in `contracts/codes/event-types.json`, each carrying the terminal ID, region, latitude and longitude, the account and the request ID: `atm.session_started`, `atm.session_rejected` (with the bank response code, and whether this attempt locked the account), `atm.session_ended` (the reason), `atm.operation` (operation `withdraw`, `deposit` or `balance`; outcome `approved` or `rejected`; amount and response codes). Each is written in the transaction of the state it describes, before `request.completed`.
- **Outcome recording.** The ATM cash routes join the money-moving route table of `shared/outcome` (types `ATM_WITHDRAW`, `ATM_DEPOSIT`, channel `ATM`). The outcome tests read `contracts/openapi/atm.yaml` too, so every ATM route needs an entry in their coverage table; their secret scan adds PINs and ATM session tokens.
- **The generator only gains what the tests need:** client methods for the ATM interface, seeding of terminals (a fixed list in configuration, one per region, with the coordinates of a city centre) and of PINs. Drivers for ATM behaviour are stage 6.

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
   - Proof: `scripts/dev/it-oracle.sh migrations`; `python3 scripts/repo.py verify --change atm-channel --group web` (the mobile app is unaffected); the `atm` suite's first test validates `atm.yaml` and `internal.yaml` as OpenAPI documents.
2. **Seeding.** Terminals; PINs on demand accounts; test helpers and generator seeding send PINs.
   - Proof: `scripts/dev/it-oracle.sh atm` (seeding: validation of region, coordinates and PIN; no PIN or hash in the response, `app.log` or the outbox) and every ordinary suite still passing.
3. **Sessions.** Start, end, replacement, expiry, lock and its expiry, rejected attempts, the credential recheck, outcome coverage.
   - Proof: `atm` in live mode: every refusal stores one rejected row with its codes; the third wrong PIN locks and ends the account's open session at another terminal, whose next withdrawal is refused `409`; eight concurrent wrong PINs on one account lock it exactly once, with exactly three counted; eight concurrent starts at one terminal leave one open session and seven ended `replaced`, each with one `atm.session_ended`; a token presented after its session ended is refused with no new event. `backfill`: after an advance of 3 minutes a token is refused, the session is `expired` once, and its event, the rejected attempt and the completion event are in one transaction; a session left open and then replaced after its expiry is ended `expired` at start + 3 minutes with a null request ID; after three wrong PINs a correct PIN is refused until the clock crosses midnight, then accepted, and the first wrong PIN of the new date counts as one. `outcomes` passes with the ATM routes in its coverage table.
4. **Ledger and cash.** `atm_cash`, withdraw and deposit, limits, idempotency, dormant reactivation.
   - Proof: `atm`: one row and one transfer per operation with the terminal ID and the row codes above; balance and daily totals; both limits, their configuration and the stored rejection; insufficient balance; replay, conflict with a 20-character key, and eight concurrent duplicates (one movement); withdrawals with different keys at two terminals racing for the last balance and for the last of the daily limit (one succeeds), including the first withdrawal of a date; a dormant account reactivated by a withdrawal and not by a balance inquiry; `transfers` still passing.
5. **Events and failure injection.** `atm.operation` for every operation; all `atm.*` events with the terminal's region and coordinates, validated against the envelope schema; responses validated against `atm.yaml`.
   - Proof: `atm`: one event of the right type per session step and operation, with coordinates equal to the seeded terminal's; the outbox refusal of `request-outcomes` (check constraint on `OUTBOX` for one request ID) applied to session start, wrong PIN, end, expiry, withdrawal, deposit and balance leaves no cash movement, daily total, dormancy change, credential count or session transition, and answers `503`.
6. **Daily close.** The `atm_cash` rule.
   - Proof: `backfill`: dates with ATM cash close with zero mismatches; a tampered `atm_cash` row is reported.
7. **Generator.** Client methods, terminal list, PIN seeding and roster field.
   - Proof: `python3 scripts/repo.py verify --change atm-channel --group python`.
8. **Whole run.** `all` includes `atm`; `e2e` seeds terminals and, after the normal run, makes ATM sessions with a wrong PIN, a withdrawal, a deposit and a balance inquiry through the generator client; `e2e.rs` checks their Oracle rows and `atm_cash` invariants, waits for the relay and finds every `atm.*` and `request.completed` event of those request IDs in MongoDB with the terminal's coordinates, and finds no PIN or ATM token in `app.log`; docs.
   - Proof: `scripts/dev/it-oracle.sh all` on the OCI host, `scripts/dev/it-oracle.sh e2e`, then `python3 scripts/repo.py verify --change atm-channel --full`.

Implementation is delegated as in `request-outcomes`: the lead agent works on the owner's Mac (edits, commits, cargo checks and the database suites against local Oracle and MongoDB containers), runs the whole-stack `e2e` and `verify --full` on the OCI host, writes one closed brief per unit, and reruns each proof before committing. Coding workers run as `codex exec --yolo -m gpt-6.1-sol -c model_reasoning_effort=low`, stay inside `write_scope` and never commit. Units run one at a time.

## Tests and proof

The unit proofs above; every event written by the `atm` suite validates against the envelope schema and every response against `atm.yaml`; the secret scan covers the PINs and ATM tokens the tests use; `repo.py verify --full`; the CI `summary` check on the pull request. No workflow file and no check registration changes.

## Risks and mitigations

- **PIN guessing.** Three attempts, then locked until the next business date, checked again on every operation; every session start runs one Argon2id verification whatever the outcome, so no refusal answers faster than another.
- **Argon2id cost at seeding.** Seeding 20,000 accounts hashes 20,000 PINs; the parameters are the passwords' (N-6), and the seeding interface already hashes passwords in batches. Stage 8 measures full-scale seeding.
- **Session state in backfill.** Expiry is computed from business time only; the `backfill` suite proves it across an advance.
- **One open session per terminal** is serialized by locking the terminal row, and a unique function-based index on the terminal for rows without an end backs it up.
- **CI time.** One more suite in `all`; if the `oracle` job's budget is exceeded, the pull request says by how much.

## Rollback or recovery

Before merge, close the pull request. After merge, revert the squash commit; migration 16 only adds objects, so a database that ran it needs `infra/oracle/reset_schema.sql` (test data only) to go back.

## Open questions

- **Abandoned sessions.** A session that nobody presents again and whose terminal is never used again stays open in `ATM_SESSION` and gets no `atm.session_ended`; it is expired by rule. Ending such sessions at the day rollover would add a job to F-27's list, which the spec does not name. Recommendation: accept the lazy expiry for this stage (terminals in the simulation are reused all the time), and decide on a rollover job when stage 6 shows how often it happens. The owner decides.
