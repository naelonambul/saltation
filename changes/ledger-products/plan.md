# Implementation Plan

## Summary

This plan delivers **stage 2** of the spec's Delivery table (ledger and products) in one change: payees, recipient inquiry, transfers to external counterparties, configurable limits, inbound deposits, term deposits, loans, password reset, the simulated clock's backfill and live modes, the day rollover (maturity, dormancy, daily close). It is API server work only: by the owner's decision, UI work starts after the API server is developed, so the mobile screens for these features are not in this change. ATM (stage 3), the card channel (stage 4), the admin interface (stage 5), generator personas and scenarios (stage 6), the web UI (stage 7) and operations (stage 8) stay out.

Stage 1 left the bank with login, balance, history and in-bank transfer in live mode only. After this change the API serves everything in acceptance criterion 8's mobile half, and the generator can drive a backfill clock across business days with the books verified each day.

### Spec acceptance criteria proved by this change

At test scale (integration tests, no 365-day backfill):

- **AC 6 (part):** daily close reports zero mismatches for every business date the tests cross, including after the concurrent-transfer test; a seeded mismatch is reported.
- **AC 8 (mobile half, API side only):** the interfaces to sign in, reset the password, view balances and history, manage payees, transfer, open and early-close a term deposit, take and repay a loan. The screens that complete AC 8 come with the UI work.
- **AC 9 (part):** in backfill mode every record's business time comes from the simulated clock; a test asserts `occurred_at` inside the clock range and `recorded_at` inside the wall-clock window.

Functional requirements covered: F-5, F-6, F-8, F-9, F-11 to F-16 (mobile limit only), F-18 and F-19 for the new operations, F-20 to F-23, F-25 to F-28, F-35 for the new events. F-7's ATM PIN, F-17, F-24 (no correction interface exists yet) and F-38's new screens wait for their stages.

### Decisions

- **One change for the whole stage,** as the spec's Delivery section says. It is large; the order of work below is cut into 12 units, each with its own proof, committed one by one.
- **No UI work in this change** (owner, 2026-11-12: UI work starts once the API server is developed). The stage-1 mobile app is left as it is; contract changes only add to what it uses, so the web checks keep passing untouched. The new mobile screens (F-38) and the admin screens are stage 7 of the Delivery table.
- **The generator only gains what the tests need:** client methods for the new interfaces, seeding of counterparties and counterparty clients, and clock commands. Personas, scenarios and the backfill driver are stage 6.
- **A new database starts in `backfill` mode** (F-26), with business time set to the wall-clock time of first start. Stage 1 started in live mode; that changes here. `clock/set` and `clock/advance` only move forward, with one exception: while the bank has no customer, `clock/set` may also move the clock back. That is how a backfill starts a year in the past: set the clock, then seed. The local stack and the end-to-end test switch to live with a generator command; switching when already live is accepted and does nothing.
- **One business time and one barrier per request.** Today the token check and the handler each read the clock. A request middleware now takes the barrier's shared side and reads business time once, before authentication; the token check, the handler and every record use that value. The rest of the request runs in its own task that owns the guard, so a dropped connection cannot release the barrier while database work continues. A clock advance, the live switch and every rollover take the exclusive side (an advance is rejected after waiting 10 seconds). The barrier is in-process, which assumes one bank process; that is what Compose runs.
- **Rollover of a date D** runs when the clock leaves D, under the exclusive barrier, in the spec's order: maturities, dormancy, daily close. Its records carry the last instant of D as business time, so they belong to D and D's close includes them. A term deposit with maturity date D is paid at the end of D; an early close during D wins.
- **Rollover recovery.** `SIM_CLOCK` gains `rolled_through`, the last date whose rollover finished, and a new table `ROLLOVER_RUN` (business date, job, started, finished) records each job. Every job is idempotent for its date: maturity pays one deposit per transaction and skips closed ones, dormancy only changes active accounts, daily close replaces the date's rows in one transaction and marks itself finished in that transaction. An advance persists the new business time only after every crossed date has rolled, so a crash leaves the clock where it was and the retry skips finished jobs. In live mode a scheduler rolls dates as wall-clock time crosses midnight, the bank catches up at start before serving, and a request whose business date is more than one day past `rolled_through` waits for the rollover. `ROLLOVER_RUN` and `rolled_through` are not in the spec's logical model; they hold no business data.
- **Bank-originated movements have no request.** F-34 and the envelope give rollover events a null request ID, so `TRANSFER.request_id` and `ACCOUNT_TRANSACTION.request_id` become nullable and the ledger accepts a movement without one. No `TRANSACTION_REQUEST` row is invented for them.
- **Daily close writes a `DAILY_CLOSE` row for every account that is open during the date,** idle ones included: the opening balance is the previous close's closing balance (0 for a new account), the closing balance is the opening plus the date's rows. A row is verified when the closing balance equals the date's last `balance_after` (by account sequence, when there are rows) and the account's stored balance, which holds because nothing can post between the end of D and its close. Every `TRANSFER` of the date is checked against F-21 by kind (two offsetting rows for in-bank `transfer`, `term_open`, `term_close`; one row for an external `transfer` or `inbound`; two same-sign rows for the loan kinds), written so that stage 4 can add `card_payment` as one more kind; a violation marks the `DAILY_CLOSE` rows of the accounts it touches, and one with no rows at all is reported in the log and the metric.
- **Every account gets a `fintech_use_num`,** term and loan accounts included, so balance and history work for them through the existing `fin_num` interfaces.
- **External transfers** go to a seeded `COUNTERPARTY` (bank code and account number). An unknown external account is rejected as an invalid account. They write one row (the sender's) and a `TRANSFER` of kind `transfer`; F-21's offsetting rule applies only when both accounts are in-bank.
- **Dormancy** applies to demand accounts only, counted from the last customer activity or, without any, the opening. A credit from someone else is accepted on a dormant account and does not reactivate it; a successful operation started by the account's owner does, in the same transaction, and a rejected one does not. Today the ledger updates last-activity time on every row and the transfer rejects a sender that is not active; the ledger changes to the initiating customer's rows only (a per-row flag), and the transfer admits a dormant sender.
- **Counterparty clients get a token from `POST /oauth/2.0/token`** (`grant_type=client_credentials`, scope `oob`), the KFTC path, on the generator listener only. The spec's interface table does not list it but its listener table routes `/oauth` there and F-15 requires client credentials. `COUNTERPARTY_CLIENT` carries the spec's scope column; this stage issues and accepts only `oob`, and `card` arrives with stage 4.
- **Money-moving `ext` requests** (deposit open and close, loan take and repay) carry a `bank_tran_id` as their idempotency key, like transfers (F-18).
- **Product terms** (seeded by migration, no interest): term deposit 6, 12 and 24 months, 100,000 to 500,000,000 KRW, early close allowed; loan 12 and 36 months, 1,000,000 to 50,000,000 KRW. A loan needs only an amount within its terms; there is no credit check.
- **Password reset:** the request answers the same for known and unknown phones; five wrong codes void the code; a successful reset unlocks the credential and ends the customer's sessions.
- **Devices** were delivered in stage 1 (stored at login, `device.new` event); this change only adds tests around them.
- **Missing stage-1 contracts are added:** `contracts/openapi/internal.yaml` and `contracts/schemas/log-envelope.schema.json` were planned in stage 1 but not written. The tests validate events against the envelope schema from now on.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `contracts/openapi/public.yaml` | password reset, recipient inquiry, payees, products, deposits, loans; transfer and account schemas extended (external recipient, term and loan accounts) |
| `contracts/openapi/inbound.yaml`, `contracts/openapi/internal.yaml` | new: client token and inbound deposit; seeding, SMS outbox, clock and mode |
| `contracts/schemas/log-envelope.schema.json` | new: the F-37 envelope |
| `contracts/codes/*.json` | response codes for the new rejections; event types |
| `bank/migrations/0007_*.sql` …, `bank/src/shared/db/migrations.rs` | `PAYEE`, `PASSWORD_RESET`, `SMS_OUTBOX`, `COUNTERPARTY_CLIENT`, `COUNTERPARTY`, `DEPOSIT_DETAIL`, `LOAN_DETAIL`, `DAILY_CLOSE`, `ROLLOVER_RUN`; `TRANSFER.payee_registered`; nullable request links; `SIM_CLOCK.rolled_through`; term and loan products and terms; simulated external banks in `INSTITUTION`, matching `contracts/codes/institutions.json`; `SIM_CLOCK` starts in backfill |
| `bank/src/shared/clock/` | backfill and live modes, set and advance, operation barrier, one-way live switch |
| `bank/src/shared/context/`, `bank/src/shared/auth/` | the request's single business time and barrier guard; client-credential tokens |
| `bank/src/shared/rollover/` | new: job registry; runs every crossed date in order; recovery; live-mode scheduler |
| `bank/src/shared/ledger/` | kinds `term_open`, `term_close`, `loan_disburse`, `loan_repay`; movements without a request; customer-initiated flag per row; reactivation of a dormant account with status history and event |
| `bank/src/shared/outbox/` | bank-originated (`SYSTEM`) events |
| `bank/src/features/auth/` | password reset and SMS outbox |
| `bank/src/features/transfers/` | recipient inquiry, external recipients, payee flag, limit from configuration |
| `bank/src/features/accounts/` | term and loan accounts in list, balance and history |
| `bank/src/features/{payees,inbound,deposits,loans,accounts_lifecycle,daily_close}/` | new features, each with router or rollover job and `sql/` |
| `bank/src/features/internal/` | seed counterparties and counterparty clients; SMS read; clock set, advance and live switch |
| `bank/src/app.rs`, `bank/src/main.rs` | routes per listener; rollover registration; configuration |
| `bank/tests/` | new suites `payees`, `inbound`, `password_reset`, `deposits`, `loans`, `backfill`; the shared helpers start logging and metrics so tests can read them; existing suites take time from the clock, accept either mode, and check transfers by kind |
| `generator/saltation_gen/{client,drivers,cli.py}`, `generator/tests/` | client methods, seeding of counterparties and clients, `clock` commands |
| `scripts/dev/it-oracle.sh` | new targets; `all` runs the phases below; `e2e` switches to live |
| `infra/compose/compose.yml`, `infra/compose/.env.example` | the limit setting, if it needs a variable |
| `docs/dev-host.md` | new test targets; starting the local stack in live mode |

## Order of work

Each unit names its proof, run from the repository root. Commit after each unit. `scripts/dev/it-oracle.sh <target>` runs the named integration suite against Oracle and MongoDB.

Test layout: the **ordinary suites** (stage 1's and `payees`, `inbound`, `password_reset`, `deposits`, `loans`) never move the clock and pass in either mode. Every test that moves the clock lives in the `backfill` suite, whose target resets the schema, sets the clock 400 days back, runs its tests in a fixed order and ends with the switch to live. The refusal to go live with a wall clock behind business time is a unit test of the clock rule with an injected wall time.

1. **Contracts and schema.** The contract files and code tables above; migrations for every new table, column and seed row.
   - Proof: `scripts/dev/it-oracle.sh migrations` (applies to an empty schema and is idempotent), and `python3 scripts/repo.py verify --change ledger-products --group web` (the existing mobile app still builds against the extended contract).
2. **Clock modes and rollover registry.** `shared/clock`: persisted mode, set, advance, barrier, live switch. `shared/rollover`: registry and `ROLLOVER_RUN`; the advance and the live scheduler call it for each crossed date. `features/internal`: `clock/set`, `clock/advance`, `mode/live`.
   - Proof: `scripts/dev/it-oracle.sh backfill` and `scripts/dev/ci-rust.sh test`. At this unit they cover: forward only, and backward only while there is no customer; an advance waits for an in-flight operation and is rejected after 10 seconds; operations wait during an advance; a request cancelled mid-transaction keeps the barrier until its database work ends; a registered test job runs once per crossed date, in order; after a job fails midway, the clock has not moved and the retry skips the finished jobs and completes; the token check and the handler see the same business time; the live switch is one-way and rolls the dates it crosses.
3. **Ledger and account lifecycle.** New kinds and the same-sign rule for loans; customer-initiated flag; dormant reactivation. `features/accounts_lifecycle`: the dormancy job.
   - Proof: `scripts/dev/it-oracle.sh transfers` still passes, and `backfill` gains: an account idle for 365 business days becomes dormant; an inbound credit leaves it dormant; the owner's transfer reactivates it; both changes are in `ACCOUNT_STATUS_HISTORY` and the outbox.
4. **Payees, recipient inquiry, external transfers, limit.**
   - Proof: `scripts/dev/it-oracle.sh payees` and `transfers`: register, list, delete; inquiry returns the holder for in-bank and external accounts and rejects unknown ones; a transfer records whether the payee was registered; an external transfer writes one row with the counterparty; the limit comes from configuration.
5. **Inbound deposits.** Client token, `/v2.0/transfer/deposit/fin_num`, seeding of clients and counterparties.
   - Proof: `scripts/dev/it-oracle.sh inbound`: a token needs valid client credentials; the route is absent on the public listener; a deposit credits once under duplicates; a customer token is refused.
6. **Password reset.**
   - Proof: `scripts/dev/it-oracle.sh password_reset`: code readable only on the generator listener; wrong and reused codes are refused, and in `backfill`, after a 5-minute advance, an expired one; success unlocks, ends sessions and lets the new password in; the code and passwords appear in no event or log line.
7. **Term deposits.** `features/deposits` with the maturity job.
   - Proof: `scripts/dev/it-oracle.sh deposits`, and in `backfill`: maturity on the right date returns exactly the principal; an early close before it wins and the maturity job skips the closed account.
8. **Loans.**
   - Proof: `scripts/dev/it-oracle.sh loans`: disbursement, partial and full repayment, over-repayment rejected, closed at zero, F-21 signs.
9. **Daily close.** `features/daily_close`: the job, its `app.log` line and metric.
   - Proof: `backfill`: after operations of every kind across several dates, every `DAILY_CLOSE` row is verified, idle accounts included; a balance tampered by test SQL on an idle account, a tampered row and a tampered `TRANSFER` are each reported as a mismatch in the table, the log and the metric. The stage-1 concurrency test runs inside this suite too, followed by a close with zero mismatches.
10. **Accounts.** Term and loan accounts in list, balance (with `maturity_date`) and history.
    - Proof: `scripts/dev/it-oracle.sh accounts`, responses validated against the contract.
11. **Generator.** Client methods, seeding, `clock show|set|advance|live`.
    - Proof: `python3 scripts/repo.py verify --change ledger-products --group python`.
12. **Whole run.** `it-oracle.sh all` runs: `mongo-users`; `migrations` (reset, so backfill mode); the ordinary suites; `backfill` (reset, ends in live); the ordinary suites again, now in live mode; `observability`. `e2e` seeds counterparties, switches to live, and runs the stage-1 browser test unchanged. Docs.
    - Proof: `scripts/dev/it-oracle.sh all`, `scripts/dev/it-oracle.sh e2e`, then `python3 scripts/repo.py verify --change ledger-products --full`.

Implementation is delegated as in `weekly-fresh-oracle`: the lead agent (Claude, model Fable, working on the OCI development host) writes one closed brief per unit and reviews each result; coding workers run as `codex exec --yolo -m gpt-6.1-sol -c model_reasoning_effort=low`, stay inside `write_scope` and never commit. The lead reruns each proof before committing. Units run one at a time, because they share `app.rs`, the ledger and the migrations. On the OCI host the integration suites share the CI runner's test database and lock, so a local run and a CI `oracle` job wait for each other.

## Tests and proof

The unit proofs above; every event written by the new suites validates against the envelope schema; `repo.py verify --full`; the CI `summary` check on the pull request, whose `oracle` job runs `it-oracle.sh all`. No workflow file and no check registration changes. `e2e` runs locally only, as in stage 1; its output is recorded on the pull request.

## Risks and mitigations

- **Size of the change.** Twelve units, each proved and committed separately; the pull request description lists them so the review can follow commit by commit.
- **Clock barrier stalls or deadlocks.** The barrier is taken once per request, before any database work, and never while holding a row lock; unit 2 tests both waiting directions, the timeout and cancellation.
- **Existing suites assumed live mode and wall-clock time.** They change to read the clock; `all` runs them once in each mode.
- **Nullable request links** weaken a constraint that stage 1 relied on. The daily close and the consistency tests check that only `SYSTEM` rows lack a request.
- **CI time.** The `oracle` job grows and runs the ordinary suites twice; the budget is under 6 minutes warm. If it is exceeded, the pull request says by how much.
- **Daily close cost at full scale** (20,000 customers, 365 dates) is not measured here; the job is set-based SQL over the date's rows, and stage 8 measures it.
- **Password reset abuse.** Identical answers for unknown phones, a five-attempt cap and Argon2id-hashed codes; the SMS outbox is on the generator listener only, with a negative test.

## Rollback or recovery

Before merge, close the pull request. After merge, revert the squash commit; the migrations only add objects, so a database that already ran them needs `infra/oracle/reset_schema.sql` (test data only) to go back.

## Open questions

- None.
