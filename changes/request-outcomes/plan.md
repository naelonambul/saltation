# Implementation Plan

## Summary

Stage 2 was merged with two known gaps, both inherited from stage 1 and disclosed in pull request #8:

- **F-35.** `request.completed` is written only by money-moving handlers that reach their own transaction. Reads, requests rejected before the handler (authentication, malformed body, unknown route), idempotent replays, login, logout, password reset and payee requests produce none, and no event carries the duration.
- **F-19.** An idempotency conflict (`409`, `A0007`) and an unauthorized money-moving request are answered but not stored in Oracle.

This change closes both before stage 3 adds another listener and more handlers, so that later stages inherit one shared mechanism. It changes no interface that a client calls: no path, request or response body changes. It is API server work only.

Functional requirements covered: F-19 (all rejections of the existing money-moving operations), F-35 (request outcome for every API request, with channel, endpoint, response codes and duration). F-36a's rule that events are never skipped is applied to the new events.

### Decisions

- **Hybrid recording, one shared contract.** A handler that commits a state change writes `request.completed` in that same transaction, after its domain events. Every other response is recorded by the request middleware in one short transaction of its own, before the response is released. Both Astra and Argon recommended this shape: middleware alone leaves a window where a committed movement has no outcome event; handlers alone miss every rejection that happens before the handler.
- **Exactly one completion event per HTTP arrival.** The request carries a shared flag. The database session sets it only after a commit that included a `request.completed` event succeeds; a rollback leaves it unset. The middleware records only when the flag is unset.
- **Which requests.** Every request to the public and generator listeners' API routes, matched or not (an unknown path or method is recorded with endpoint `unmatched`). Not recorded: `/healthz`, `/metrics`, and `/internal/**`, which is the simulation's control plane (seeding, clock, outbox status), not the bank's API.
- **Event content.** The envelope fields as today, plus in `payload`: `endpoint` (the route template, never the raw path or query string), `method`, `http_status`, `duration_ms` and, where known, `request_type`. `customer_no`, `session_id` and `device_id` are set once authentication has resolved them; `account_no` is set when the handler has resolved one account. No request or response body, header value, password, PIN, reset code, token or client secret is ever placed in an event.
- **Duration** is the time from the request's arrival to the moment its outcome is recorded. It cannot include the time to send the response, because the event is committed before that. Full HTTP latency stays in the existing request log and metrics.
- **Business time.** The middleware records while the request still holds the clock barrier's shared side, with the same business time the handler used.
- **Rejected attempts are stored in `TRANSACTION_REQUEST`** (F-19), the table the spec's data model and scenario table already name. The idempotency columns (`idem_scope`, `idem_key`, `request_hash`) become nullable for rejected rows only, and a check constraint requires them on every other row. The unique constraint on `(idem_scope, idem_key)` stays, so a rejected attempt can never claim or overwrite a key. Stored this way:
  - an idempotency conflict on transfer, inbound deposit, term deposit open or close, loan take or repay: `REJECTED`, `A0007`, the attempted key in `bank_tran_id`, written with its completion event in one transaction;
  - an unauthorized (`401` or `403`) request to one of those operations that is rejected before the handler: `REJECTED` with its response code, written by the middleware with the completion event.
  The row's `request_id` is the request's ID. If that ID already has a row (a client reused it), the attempt is stored under a new bank-generated ID and the completion event's payload carries it as `attempt_id`.
- **Not stored as attempts:** malformed requests (`400`, `413`, `408`) and unknown routes get the completion event only, as F-42 already rules for card messages. An identical replay returns the stored result and gets a completion event with `replayed: true`, and no new row.
- **Failure semantics: fail closed.** If the outcome cannot be stored, a response below `500` is replaced with a retryable `503` and the handler's transaction, if any, is rolled back with it (it is the same transaction). A response that is already `500` or above is still attempted; if that also fails, the response is sent as it is, a counter `bank_request_outcomes_unrecorded_total` is incremented and a warning is logged. A request that fails before business time can be read (the clock barrier is busy) has no event for the same reason.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

- `bank/migrations/0014_request_attempts.sql` (new), `bank/src/shared/db/migrations.rs`, `bank/tests/migrations.rs`: nullable idempotency columns and the check constraint.
- `bank/src/shared/outcome/` (new module): the outcome metadata a response carries, the shared recorded flag, the route-to-`request_type` table for money-moving routes, and the fallback recording with its SQL files.
- `bank/src/shared/context/mod.rs`: arrival time on the request context; the operation middleware calls the fallback inside the task that owns the clock guard, and also covers body-size and body-timeout rejections.
- `bank/src/shared/api/mod.rs`: `ok`, `respond` and `ApiError` attach response codes to the response as typed metadata, so the middleware never parses a body.
- `bank/src/shared/auth/mod.rs`: successful authentication leaves the resolved identity for the middleware.
- `bank/src/shared/db/mod.rs`, `bank/src/shared/outbox/mod.rs`: the session notes a recorded completion event and publishes it after a successful commit.
- `bank/src/app.rs`: fallback handlers for unknown routes and methods inside the recorded layers.
- `bank/src/features/{auth,password_reset,payees}/mod.rs`: completion event in the existing transaction of each state-changing request.
- `bank/src/features/{transfers,inbound,deposits,loans}/mod.rs` and their `sql/`: `duration_ms`, `method` and `http_status` on the existing completion events; conflict attempts stored; replays marked.
- `bank/src/features/accounts/mod.rs`, and the read handlers in `deposits`, `loans`, `payees`, `transfers`: attach the resolved account to the response metadata.
- `bank/src/shared/telemetry/mod.rs`: the unrecorded counter.
- `bank/tests/outcomes.rs` (new), `bank/tests/sql/outcome_*.sql` (new), `bank/tests/common/mod.rs`; existing suites where an expectation changes (`bank/tests/inbound.rs` expects no completion event on a replay today).
- `contracts/codes/event-types.json`, `contracts/schemas/log-envelope.schema.json`: the completion event's payload fields.
- `scripts/dev/it-oracle.sh`: target `outcomes`, added to `all`. `docs/dev-host.md`: the target and a short "Request outcomes" section.

## Order of work

Each unit is committed after the coordinator re-runs its proof. Units 2 to 4 share files and run in sequence.

1. **Contracts and migration.** Files: `bank/migrations/0014_request_attempts.sql`, `bank/src/shared/db/migrations.rs`, `bank/tests/migrations.rs`, `contracts/**`. Proof: `scripts/dev/it-oracle.sh migrations` and `scripts/dev/it-oracle.sh backfill`.
2. **Shared mechanism and fallback.** Files: `bank/src/shared/**`, `bank/src/app.rs`, `bank/tests/outcomes.rs`, `bank/tests/sql/outcome_*.sql`, `bank/tests/common/mod.rs`, `scripts/dev/it-oracle.sh`. After this unit every request has exactly one completion event: handlers that already write one keep doing so, everything else comes from the middleware. Proof: `scripts/dev/it-oracle.sh outcomes` (tests 1, 2, 5 and 6 below), plus `listeners`, `auth` and `accounts`.
3. **State-changing handlers.** Files: `bank/src/features/{auth,password_reset,payees,transfers,inbound,deposits,loans,accounts}/**`, `bank/tests/outcomes.rs` and the suites of those features. Completion events move into the handler's transaction for login, logout, password reset and payee changes; all completion events gain the new payload fields; reads attach their account. Proof: `scripts/dev/it-oracle.sh outcomes` (tests 3 and 7), plus `auth`, `password_reset`, `payees`, `transfers`, `inbound`, `deposits`, `loans`, `accounts`.
4. **Rejected attempts (F-19).** Files: `bank/src/features/{transfers,inbound,deposits,loans}/**`, `bank/src/shared/outcome/**`, `bank/tests/outcomes.rs` and those features' suites. Proof: `scripts/dev/it-oracle.sh outcomes` (test 4), plus `transfers`, `inbound`, `deposits`, `loans`.
5. **Whole run and documentation.** Files: `docs/dev-host.md`. Proof: `scripts/dev/it-oracle.sh all`, `scripts/dev/it-oracle.sh e2e`, `python3 scripts/repo.py verify --change request-outcomes --full`.

## Tests and proof

`bank/tests/outcomes.rs`, run against Oracle by `scripts/dev/it-oracle.sh outcomes`:

1. **Coverage.** A table lists every API route of both listeners with one successful call. For each, exactly one `request.completed` event exists for the request ID, with the route template, method, HTTP status, response code, channel, listener and a non-negative `duration_ms`. A check compares the table with the router's routes, so a route added later without a table entry fails the test.
2. **Rejections before the handler.** Missing token, expired token, wrong scope, malformed JSON, oversized body, unknown path and wrong method each give exactly one event with the outcome `rejected` and the response's codes.
3. **Same transaction.** For login, logout, password reset, payee register and delete, and every money-moving operation: the completion event's `event_id` is greater than the domain events of the same request, and an injected failure of the completion insert leaves no state change, no domain event and a `503`.
4. **Rejected attempts.** A conflict on each money-moving operation stores one `REJECTED` row with `A0007` and null idempotency columns, leaves the original row, its response and all balances unchanged, and a later identical replay of the original still returns the original result. An unauthorized call to each stores one `REJECTED` row. A reused request ID is stored under a new ID named in the event. Malformed requests store no row. Concurrent identical requests produce one movement and one completion event per arrival.
5. **Fail closed.** With outbox inserts failing for one request ID (a trigger the test creates and drops), a read returns `503` and no event; the counter does not move for that case, and it moves when a `500` response cannot be recorded.
6. **No secrets, no bodies.** Across the whole suite no event contains a password, PIN, reset code, token, client secret, raw query string or request body field.
7. **Content.** Events of authenticated requests carry the customer, session and device; account reads and money-moving requests carry the account; times are the request's business time.

Existing suites keep passing with their assertions unchanged, except where they assert the old gap (no completion event on an inbound replay); those assertions are changed to the new rule, not removed.

The pull request reports the duration of `scripts/dev/it-oracle.sh e2e` before and after the change, since every read gains one short transaction.

## Risks and mitigations

- **Double or missing events.** One flag, set only after commit, decides who records; test 1 asserts exactly one for every route, tests 2 and 4 for rejections and concurrency.
- **A response sent although its event was lost.** Fail closed for responses below `500`; counter and warning above. A database that is unavailable cannot record its own failure; this boundary is stated here and in the documentation rather than hidden.
- **Throughput.** One more short pooled transaction per read and per rejection. At the spec's live rates (5 to 50 requests per second) this is small; the e2e duration is reported, and if it grows by more than 20% the coordinator stops and reports before continuing.
- **Clock barrier held longer.** The fallback runs inside the existing task that owns the shared side, so a rollover waits for the outcome insert as it already waits for the handler; no new lock order is introduced.
- **The relaxed columns weaken the ledger's idempotency.** The check constraint keeps them mandatory for every row that is not `REJECTED`; the migration test asserts the constraint rejects a `DONE` row without a key.
- **Leaking data into events.** Only allowlisted metadata is recorded, from typed values; test 6 scans the events.

## Rollback or recovery

The change is one squashed commit. Reverting the code restores the previous behaviour; the migration is forward-only and harmless when left in place (the old code always writes the three columns). To remove it, a new change deletes rejected rows with null keys and restores `NOT NULL`. Completion events already delivered to MongoDB are additive and need no cleanup.

## Open questions

1. **Where rejected attempts live.** This plan stores them in `TRANSACTION_REQUEST` with null idempotency columns (Argon's advice; it keeps the spec's data model). Astra advised a new `REQUEST_ATTEMPT` table with one row per HTTP arrival, which is cleaner but is a table the spec does not have and would need a spec change first. Approving this plan chooses the first.
2. **`/internal/**` is not recorded.** The spec says "every API request"; this plan reads that as the bank's API, not the simulation's control plane.
3. **No event when Oracle cannot store it** for a response that is already a server error. The spec's "events are never skipped" cannot hold when the event store itself is down; the plan counts and logs those.
