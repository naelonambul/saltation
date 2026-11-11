# Specification

## Summary

This specification turns the approved intent into a buildable design for the **data foundation**: the Rust bank, its Open-Banking-shaped API, the simulated ATM channel, the customer mobile web app, the analyst admin web app, the Python synthetic generator with ground truth, the Oracle ledger, the MongoDB business event logs, operational monitoring, and the Docker Compose environment on the OCI development host.

Real-time detection is out of scope for the `product-init` change. This specification fixes only the hooks detection will need (a transactional outbox, request and session correlation, an `fds_detect`-shaped alert contract) so that a later change can add detection without reshaping the data foundation.

References used for interface and data shapes:

- KFTC Open Banking user-institution API specification v3.3.6 (오픈뱅킹공동업무 이용기관 API명세서 v3.3.6, KFTC Open API portal, materials library). Paths, field names, types and lengths are followed where an equivalent exists; the document itself is not copied into the repository.
- The Korean financial MyData bank-sector data model, through the owner's analysis of a MyData-based target logical data model (customer, institution, product, integrated account, deposit account detail and transaction, loan account detail and transaction). Code-level values are pinned once the official MyData standard API document is obtained (see Open questions).

## Requirements

### Functional

**Customers, sessions and authentication**

- F-1. The bank stores customers with a synthetic connecting-information hash (`ci_hash`) in place of a resident registration number, a name, a mobile phone number, an address and a customer type (individual, sole proprietor). Persona and fraud role are never stored in the bank.
- F-2. A customer signs in with phone number and password (local login, simplified). Success issues a bearer access token with scopes `login inquiry transfer`, returns `user_seq_no`, and opens an authentication session. Tokens and sessions expire after 30 minutes of business-time inactivity.
- F-3. Every login attempt, including attempts for unknown phone numbers, is stored in Oracle with outcome, device, session (if any), source IP and region. Five consecutive failures lock the credential for 30 minutes of business time.
- F-4. Devices are identified by a client-supplied device identifier and stored per customer with first-seen and last-seen times; the first successful login from an unseen device is a new-device event.
- F-5. Password reset: the customer requests a reset; the bank issues a 6-digit code to a simulated SMS outbox; the customer submits phone, code and new password. Codes expire after 5 minutes of business time. The SMS outbox is readable only on the internal listener (by the generator).

**Accounts and products**

- F-6. Products: demand deposit (입출금), term deposit (정기예금), loan (대출). Each product has an immutable terms version (term in months, minimum and maximum amounts, early close allowed). No product carries an interest rate.
- F-7. Opening a demand deposit account assigns an account number, a 4-digit ATM PIN and an Open Banking `fintech_use_num` (24 characters) bound to that account.
- F-8. Term deposit: opening moves the principal from a linked demand deposit account to the term deposit account. Early close (by the customer) or maturity (by the day rollover) moves exactly the principal back to the linked account and closes the term account. Whichever runs first wins; the other is rejected because the account is closed.
- F-9. Loan: disbursement credits the principal to a linked demand deposit account and sets the loan account's balance (amount owed) to the principal. Repayment moves the repaid amount from the linked account and reduces the amount owed by exactly that amount. Repayment above the amount owed is rejected; the loan account closes at zero.
- F-10. All amounts are whole Korean won, `NUMBER(15,0)`, between 1 and 999,999,999,999 per operation. KRW only.
- F-11. Account status is active, dormant or closed, with a status history. An account with no customer-initiated transaction for 365 business days becomes dormant in the day rollover; a dormant account becomes active again on its next customer-initiated transaction, and both changes are recorded.

**Transfers, payees, inbound funds and cash**

- F-12. A customer registers, lists and deletes payees (bank code, account number, holder name, alias); registration time, channel and session are recorded.
- F-13. A recipient inquiry (`/v2.0/inquiry/receive`) returns the destination holder name.
- F-14. A transfer moves money from the customer's demand deposit account to another in-bank account or to a simulated external counterparty account. Transfers to unregistered payees are allowed and recorded as such. A transfer to the same account is rejected.
- F-15. Inbound external funds (salaries, merchant settlements, unrelated external senders) arrive through `/v2.0/transfer/deposit/fin_num`, called by a simulated counterparty client authenticated with client credentials (scope `oob`).
- F-16. Limits (defaults, configurable): mobile transfers 50,000,000 KRW per account per business day; ATM cash withdrawal 1,000,000 KRW per transaction and 6,000,000 KRW per account per business day; no limit on cash deposits. Exceeding a limit rejects the request.
- F-17. The simulated ATM authenticates an ATM session with terminal ID, account number and PIN (three consecutive wrong PINs lock ATM access for the account until the next business day), then performs cash withdrawal, cash deposit and balance inquiry. ATM sessions expire after 3 minutes of business time.
- F-18. Every money-moving request carries an idempotency key, scoped by client (customer, counterparty client or ATM terminal); for Open Banking-shaped requests the key is `bank_tran_id`. The bank stores a hash of the canonical request. A repeat with the same key and hash returns the original result; with a different hash it is rejected. Concurrent duplicates resolve through the unique constraint: one executes, the others return its result.
- F-19. Rejected attempts (insufficient balance, limit exceeded, invalid or closed account, same-account transfer, locked credential, wrong PIN, unauthorized, idempotency conflict) are stored in Oracle with their response codes and no balance effect.

**Ledger (simple, single-entry)**

- F-20. Every money movement writes one transaction row per in-bank account it affects, with a signed amount and the balance after, and one `TRANSFER` record of a kind: `transfer`, `inbound`, `atm_cash`, `term_open`, `term_close`, `loan_disburse`, `loan_repay`. Counterparty bank code, account number and name are stored where they exist (null for cash).
- F-21. Sign rules: for demand and term accounts the balance is money held; for loan accounts the balance is money owed. `transfer`, `term_open` and `term_close` rows between two in-bank accounts have equal and opposite amounts; `loan_disburse` and `loan_repay` rows on the demand and the loan account have equal amounts with the same sign.
- F-22. A single shared ledger component is the only code that writes account balances and transaction rows. Every operation that touches more than one account locks all of them with `SELECT ... FOR UPDATE` in ascending account-ID order, and writes rows, balances, daily totals and outbox events in one database transaction.
- F-23. Daily close, per business date and account: opening balance plus the sum of that date's rows equals the last `balance_after` of that date; every `TRANSFER` satisfies F-21. Results are stored, and a mismatch is reported in `app.log`, a metric and the admin web app.
- F-24. Corrections are new reversing rows referencing the original; transaction rows are never updated or deleted.

**Simulated clock and day rollover**

- F-25. The bank reads business time only from its simulated clock, whose state is persisted in Oracle. Each operation reads business time once, at its start, and uses that value for all its records.
- F-26. Modes: `backfill` (initial) and `live`. In backfill the generator sets and advances the clock through the internal interface, forward only. Advancing waits until in-flight operations finish (rejecting the advance after 10 seconds) and blocks new operations until it completes. The switch to live is one-way and sets the clock to wall-clock time, which must not be earlier than the current business time. In live mode the clock follows wall-clock time.
- F-27. Day rollover runs once for every business date crossed, in date order, in both modes: term deposit maturities, dormancy, then daily close. Features register their rollover jobs with a shared rollover registry at startup; the clock advance (backfill) and a scheduler (live) invoke the registry.
- F-28. Every business record carries business time; every record also carries system (wall-clock) recording time. No business time column has a database default.

**Seeding and generator**

- F-29. The generator creates customers, credentials, accounts, ATM terminals, counterparty clients, counterparties and analyst accounts through the internal seeding interface, not through customer channels.
- F-30. The generator drives all customer, ATM and counterparty activity through the same public, ATM and inbound-deposit interface contracts the bank exposes, on the generator listener (see Runtime topology), in backfill (365 business days ending before the switch to live) and then in live mode.
- F-31. Each generator request carries an opaque request ID (ULID) and simulated context: source IP, region and device ID. The bank stores the request ID on every request record, transaction row and log event the request produces, and trusts simulated context only on the generator listener.
- F-32. Personas: office worker, student, self-employed, elderly, each with a behaviour model (salary days, recurring payments, cash habits, active hours, devices, locations). Fraud actors play scenarios A to F; benign lookalikes play legitimate travel, new phone, one-off large purchase and shared household network (see the scenario matrix).
- F-33. Scenario frequencies start from published statistics and are scaled by a configurable amplification factor recorded in each run manifest. Defaults: population 20,000 customers; amplification and yearly instance counts as in the scenario matrix, with sources documented in `generator/config`.
- F-34. Ground truth: before sending a request, the generator records a label for its request ID per subject the request involves. The subject is an account number for account events and the customer number for customer-level events (login, password reset, device, payee). Labels are `normal`, `lookalike` or `A`..`F` with a scenario instance ID. A B-to-C transfer has a B label for the sender's account and a C label for the receiver's account. Bank-originated events (rollover jobs) have no request ID and are not labelled. Requests arriving through the public listener are human sessions and are excluded from labelled datasets.

**Business event logs**

- F-35. Every business event is written to the `OUTBOX` table in the same Oracle transaction as the state it describes, including rejected attempts and authentication events: login success and failure, logout, session expiry, password reset request and completion, new device, payee change, request outcome (every API request, with channel, endpoint, response codes and duration), ATM session and operation, status changes, rollover jobs.
- F-36. An outbox relay delivers events to MongoDB `bank_logs` at least once, upserting on `_id` = `event_id`, so each event exists there exactly once after delivery. Relay lag is a metric; verification requires no undelivered event older than 60 seconds.
- F-37. Every event uses the versioned JSON envelope (see Interfaces) with string identifiers and ISO-8601 timestamps only.

**Mobile web app (customer)**

- F-38. Sign in; view accounts, balances, term deposits and loans; view transaction history with pagination; manage payees; recipient inquiry and transfer; open and early-close a term deposit; take and repay a loan; reset password.

**Admin web app (analyst)**

- F-39. Analyst sign-in (analyst accounts, seeded). Search customers by name, phone or customer number; view a customer's accounts and balances, term deposits, loans, payees, devices, recent sessions and login attempts; view an account's transactions with filters (date range, channel, direction, amount); view a transfer's rows and request outcome; view daily close results. Analysts cannot move money.

### Non-functional

All performance figures are server-side (request received to response sent) on the development host, with Saltation within its N-7 budget.

- N-1. **Transfer latency:** at a sustained 50 transfers per second for 10 minutes between random in-bank accounts: p50 ≤ 50 ms, p95 ≤ 300 ms, p99 ≤ 1,000 ms; row-lock wait p95 ≤ 50 ms; at least 99.9% of requests succeed (business rejections excluded).
- N-2. **Hot account:** 50 transfers per second into one account from at least 200 sender accounts for 5 minutes: p95 ≤ 500 ms, same success rate.
- N-3. **Saturation point:** stepped load of 25, 50, 100 and 200 transfers per second, 3 minutes per step; the first step whose p95 exceeds 300 ms is above 100 per second; the limiting resource (lock wait, disk, CPU, connection pool) is recorded.
- N-4. **Backfill throughput:** at least 200 generated operations per second sustained, where an operation is one API request; the default population produces about 7.5 million requests per business year (about 375 per customer, including logins and inquiries), so the backfill completes within 24 hours.
- N-5. **Correctness:** zero daily-close mismatches, zero duplicate money movements and zero `TRANSFER` records violating F-21, after every load test and run.
- N-6. **Security cost settings:** password and PIN hashing uses Argon2id with parameters fixed in configuration and sized so that login stays within N-4 (the plan measures and records them).
- N-7. **Resource budget:** Saltation containers together have CPU limits of at most 3 vCPU and memory limits of at most 14.5 GB; each container has its own CPU and memory limit. Initial memory: Oracle 6 GB (SGA 4 GB, `PGA_AGGREGATE_LIMIT` 1.5 GB), MongoDB 3 GB (WiredTiger cache 1.5 GB), truth MongoDB 512 MB, bank 1 GB, generator 1 GB, nginx 128 MB, cloudflared 128 MB, Alloy 256 MB, Loki 768 MB, Prometheus 768 MB, Grafana 256 MB. Oracle redo logs are sized for the backfill write rate (initially three groups of 1 GB).
- N-8. **Co-tenant safety:** during the backfill, the 24-hour live run and load tests, no Saltation container is OOM-killed or restarted, and the co-tenant containers (`sonarqube-sonarqube-1`, `postgresql-postgresql-1`, `qdrant-qdrant-1`, `npm`, `jenkins`, `cloudflared-oci-podobank-api`) have zero restarts and keep their health status.
- N-9. **Storage and retention:** Saltation uses at most 70 GB of the block volume. Retention: Oracle keeps all data; MongoDB `bank_logs` 18 months; delivered outbox rows 7 days; Loki 14 days; Prometheus 15 days; `app.log` rotated daily, 7 files kept. An alert fires when Saltation's data volumes exceed 80% of the budget. Data beyond Oracle's budget is archived in a later change.
- N-10. **Portability:** every image is published for, or (Oracle) buildable from documented steps on, linux/arm64 and linux/amd64.
- N-11. **Operability:** one `docker compose up` starts all server-side services after documented prerequisites (the Oracle image built locally, secrets and the Cloudflare tunnel token in `.env`); every service has a health check, and all report healthy within 10 minutes of start (Oracle first start excluded). The mobile web app needs no installation.
- N-12. **Observability:** the bank exposes Prometheus metrics (request rate and latency histograms per endpoint, per-statement latency histograms by statement name, transfer outcomes, row-lock wait, DB pool usage, outbox lag, daily-close mismatches) on a separate metrics listener, and writes operational logs as JSON lines to `app.log`: one line per request with request ID, session ID, endpoint, status, duration, statement count and slowest statement. Per-statement lines with SQL text are written only at debug level. Grafana dashboards show N-1 percentiles, error rates, outbox lag and container resources.
- N-13. **Partitioning is optional.** The design meets N-1 to N-5 without Oracle Partitioning. Tables carry business date so monthly interval partitioning can be enabled later if the license permits it.

### Scenario matrix

Counts are scenario instances. "Year" is the 365-day backfill at the default amplification; "live" is the minimum in the 24-hour live run. Counts per year at amplification 1 and the default amplification factor (20) are set in `generator/config` with their sources; the yearly counts below are the configured targets.

| Scenario | Generator actions | Required records | Year | Live |
|---|---|---|---|---|
| A account takeover | failed-login burst from a new device and foreign or new region; password reset; login; payee registration; high-value transfer to the new payee | `LOGIN_ATTEMPT` failures, `PASSWORD_RESET`, `DEVICE` new, `PAYEE`, `TRANSACTION_REQUEST`, `TRANSFER` + rows; events `login.failed`, `password_reset.*`, `device.new`, `payee.added`, `request.completed` | 200 | 2 |
| B voice-phishing victim | elderly-leaning victim, own device; early close of a term deposit or a loan; transfers to new payees that are mule accounts; large ATM withdrawals | `TRANSFER` (`term_close` or `loan_disburse`), `transfer` rows to C accounts, `atm_cash` rows, `ATM_SESSION`; events as above plus `atm.*` | 200 | 2 |
| C money mule | new or dormant account; inbound transfers from B victims and external senders; ATM withdrawals soon after; devices or IPs shared with other mules | inbound `transfer` and `inbound` rows, `atm_cash` rows, `ACCOUNT_STATUS_HISTORY` (dormant→active), shared `DEVICE` IDs | 100 accounts | 2 |
| D structuring | cash deposits split across at least 3 business days and at least 2 accounts, each account's daily cash total between 9,000,000 and 9,999,999 KRW (simulated reporting threshold 10,000,000 KRW per account per business day) | `atm_cash` rows, `DAILY_ACCOUNT_TOTAL` cash-in | 100 | 2 |
| E velocity or amount anomaly | standalone: a burst of transfers or one amount far above the customer's baseline, no A–D pattern | `transfer` rows | 300 | 2 |
| F time or location anomaly | standalone: activity at hours the persona never uses, or from a foreign or unusual region, no A–D pattern | `LOGIN_ATTEMPT`, `AUTH_SESSION` with region, `request.completed` events | 300 | 2 |
| Benign lookalikes | travel (foreign region), new phone, one-off large purchase, shared household network | as for E/F and A without fraud steps | 1,500 | 5 |

Scenario instances never overlap a single subject at the same time; every instance has one primary label.

## User experience and behavior

- **Mobile web (customer, Korean UI):** login (phone, password) and password reset; home with account cards and balances; account detail with transaction list (date, time, `print_content`, amount, balance after); transfer flow: choose payee or enter bank and account → recipient inquiry shows holder name → amount → confirm → result with response message; payees; products (open term deposit, take loan); term deposit and loan detail with early close and repay.
- **Admin web (analyst, Korean UI):** login; customer search; customer 360 view (profile, accounts, deposits, loans, payees, devices, recent sessions and login attempts); account transactions with filters and CSV export; transfer detail with rows and request outcome; daily close report.
- **Errors:** every response carries `rsp_code` and `rsp_message`; bank-level outcomes also carry `bank_rsp_code` and `bank_rsp_message`. Messages are Korean.
- **Time display:** KST; customers and analysts see business time.

## Architecture and design

### Top-level modules and ownership

The code is organised **by feature** (vertical slices): the Rust server follows vertical-slice / package-by-feature, the React apps follow the Bulletproof React layout, and the generator groups code by persona and scenario.

| Module | Path | Owns | Language |
|---|---|---|---|
| Contracts | `contracts/` | OpenAPI documents for the public, inbound-deposit, ATM, admin and internal interfaces; the log envelope JSON Schema; code tables | YAML, JSON |
| Bank server | `bank/` | The Rust API server (layout below) | Rust, SQL |
| Web | `web/apps/mobile`, `web/apps/admin`, `web/packages/api-client` | Customer and analyst SPAs; a typed client generated from `contracts/` | TypeScript, React |
| Generator | `generator/` | Personas, scenario engine, seed, backfill and live drivers, truth writer, run manifests | Python |
| Infra | `infra/compose`, `infra/oracle`, `infra/nginx`, `infra/cloudflare`, `infra/observability`, `infra/lab` | Compose files, Oracle image build steps and init scripts, nginx config, tunnel ingress rules, Alloy/Loki/Prometheus/Grafana config and dashboards, later Terraform/Ansible for the VMware lab | YAML, shell, HCL |
| Load tests | `loadtest/` | N-1 to N-5 tests and reports | Python |

**Bank server layout**

```text
bank/
├── migrations/            # numbered plain-SQL schema migrations
└── src/
    ├── main.rs            # config, wiring, listeners
    ├── app.rs             # composes feature routers per listener; registers rollover jobs
    ├── shared/
    │   ├── db/            # pool, transaction helper, statement runner (timing, logging)
    │   ├── ledger/        # the only writer of balances, transaction rows, TRANSFER, daily totals
    │   ├── clock/         # simulated clock, mode, operation barrier
    │   ├── rollover/      # rollover job registry
    │   ├── outbox/        # event envelope and outbox insert
    │   ├── auth/          # token, session and client-credential extractors
    │   ├── context/       # request ID and simulated context (generator listener only)
    │   ├── api/           # response envelope, Open Banking codes, errors
    │   └── telemetry/     # app.log writer, metrics
    └── features/
        ├── auth/          # login, logout, sessions, devices, password reset, SMS outbox
        ├── accounts/      # user info, account list, balance, transaction history
        ├── transfers/     # recipient inquiry, transfer, transfer result, limits
        ├── inbound/       # inbound deposits from counterparty clients
        ├── payees/
        ├── deposits/      # term deposits, maturity rollover job
        ├── loans/
        ├── atm/           # ATM sessions and cash
        ├── accounts_lifecycle/  # dormancy rollover job, status history
        ├── daily_close/   # daily close rollover job and reports
        ├── admin/         # analyst login and read-only queries
        ├── internal/      # seeding, clock control, mode switch
        └── outbox_relay/  # delivery of outbox events to MongoDB
```

Each feature folder holds its router, handlers, rules, repository and its own `sql/` directory of hand-written Oracle statements.

**Web app layout** (both apps): `src/app/` → `src/features/<feature>/` (`api/`, `components/`, `hooks/`, `types.ts`, `index.ts`) → `src/shared/`. Mobile features: auth, accounts, transfers, payees, deposits, loans. Admin features: auth, customer-lookup, account-transactions, transfer-detail, daily-close.

**Generator layout:** `generator/saltation_gen/` with `personas/`, `scenarios/` (one module per scenario and per lookalike), `drivers/` (seed, backfill, live), `client/` (generated from `contracts/`), `truth/`, `config/`.

### Dependency direction

- Inside each app: `app` → `features` → `shared`, one way only. A feature never imports another feature; shared needs move to `shared`. `shared` never imports `features`. Cross-feature effects go through `shared` (ledger, outbox, rollover registry).
- `web` and `generator` use the bank only through `contracts/` interfaces. The generator reads and writes only its own truth store; it never connects to Oracle or `bank_logs`. The bank never connects to the truth store.
- **Where new features go:** a new banking capability is a new folder under `bank/src/features/` with its interface first added to `contracts/`; a new screen is a new folder under the app's `src/features/`. Detection, when added, is a new top-level module (`detect/`) consuming `bank_logs` and read-only Oracle views; it does not import bank code. A later Python data service (pandas and NumPy) is a new top-level module (`analytics/`) reading data stores read-only.
- These boundaries are enforced by automated import checks for the bank, the web apps and the generator (registered by the plan).

### Runtime topology

```text
browser ─HTTPS─▶ Cloudflare (TLS, Workers VPC) ─tunnel─▶ nginx: mobile web, admin web (static only)
browser ─HTTPS─▶ Cloudflare (TLS, Workers VPC) ─tunnel─▶ bank public listener: /oauth, /v2.0 (customer), /admin
generator ─HTTP─▶ bank generator listener: /oauth, /v2.0, /atm, /internal (internal network only)
bank ─▶ Oracle (structured data, outbox) ─ relay ─▶ MongoDB bank_logs (business events)
bank ─ app.log ─▶ Alloy ─▶ Loki ─▶ Grafana;  bank metrics listener ─▶ Prometheus ─▶ Grafana
generator ─▶ truth MongoDB (separate instance)
```

**Bank listeners**

| Listener | Network | Routes | Context |
|---|---|---|---|
| Public (8080) | `edge` | `/oauth/2.0/revoke`, customer `/v2.0/**` except `/v2.0/transfer/deposit/fin_num`, `/admin/v1/**` | IP and region from Cloudflare headers; every session is human |
| Generator (8081) | `internal` | everything on the public listener plus `/v2.0/transfer/deposit/fin_num`, `/atm/v1/**`, `/internal/v1/**` | Simulated context headers trusted; sessions are not human |
| Metrics (9100) | `obs` | `/metrics` | — |

**Compose services**

| Service | Image | Networks |
|---|---|---|
| `oracle` | locally built Oracle Database 19c EE | `data` |
| `mongo` | official MongoDB, `--auth`, user for `bank_logs` only | `data` |
| `mongo-truth` | official MongoDB, `--auth`, separate instance | `truth` |
| `bank` | built from `bank/`; HTTP listeners above; `app.log` on the `applogs` volume | `data`, `edge`, `internal`, `obs` |
| `web` | nginx serving the two SPAs; no API proxying | `edge` |
| `cloudflared` | tunnel connector; ingress allow-list: `web`, and the bank public listener | `edge` |
| `generator` | built from `generator/` | `internal`, `truth` |
| `alloy` | tails `app.log` (read-only volume mount) | `obs` |
| `loki`, `prometheus`, `grafana` | official images | `obs` |

- Two log streams: business events (F-35) go to MongoDB `bank_logs` as product data; operational logs (N-12) go to Loki for monitoring. Both carry request ID and session ID. Access and application events required by the intent are business events in `bank_logs` (`request.completed`, authentication events).
- TLS is terminated by Cloudflare. No Saltation port is published on the host. `/admin/v1/**` and the admin web origin are behind Cloudflare Access. Grafana is reached through an SSH tunnel.
- Local testing: the `local` Compose profile omits `cloudflared` and publishes `web` and the bank public listener on localhost over plain HTTP; the CORS allow-list contains the local origins only in that profile. Otherwise the CORS allow-list contains the two SPA origins.

### Data access: plain Oracle SQL, no ORM

- No ORM and no query builder. Every statement is hand-written Oracle SQL in its own file under the owning feature's `sql/` directory (shared ledger statements under `shared/ledger/sql/`), embedded at compile time, executed through the `oracle` crate with bind variables only; SQL is never assembled from strings.
- Each SQL file starts with a comment giving the statement name, purpose and expected access path, so the exact query can be read in the repository and checked with `EXPLAIN PLAN`.
- Schema changes are numbered SQL migration files under `bank/migrations/`, recorded in a migration history table.
- Credential values (passwords, PINs, codes, tokens and their hashes) are never logged.

### Money operation processing (one Oracle transaction)

1. Read business time once; pass the clock barrier.
2. Insert the request into `TRANSACTION_REQUEST` with idempotency scope, key and request hash. On a unique-constraint conflict, wait for the other transaction, then return its stored result if the hash matches, or reject if it differs.
3. Lock every in-bank account involved, in ascending account-ID order, with `SELECT ... FOR UPDATE`; lock their `DAILY_ACCOUNT_TOTAL` rows for the business date.
4. Validate status, ownership, balance and limits.
5. Through the shared ledger: insert `TRANSFER` and transaction rows, update balances, daily totals and last-activity time; status change if dormant.
6. Insert outbox events; update the request outcome; commit. A business rejection records the outcome and its outbox event with no ledger effect and commits.

## Interfaces, data, and dependencies

### Public interface (Open Banking-shaped)

Paths, field names, types and lengths follow KFTC v3.3.6 where an equivalent exists. Saltation acts as hub and provider bank at once, with a synthetic `bank_code_std` recorded in `contracts/codes`. Requests may carry `X-Request-Id`; the bank generates one when absent and returns it in every response.

| Function | Method and path | Notes |
|---|---|---|
| Login (ext) | `POST /v2.0/ext/auth/login` | phone, password, device ID → `access_token`, `expires_in`, `scope`, `user_seq_no` |
| Password reset (ext) | `POST /v2.0/ext/auth/password_reset/request`, `POST /v2.0/ext/auth/password_reset/confirm` | F-5 |
| Logout | `POST /oauth/2.0/revoke` | |
| User info | `GET /v2.0/user/me` | accounts with `fintech_use_num`, `account_num_masked` |
| Account list | `GET /v2.0/account/list` | demand, term and loan accounts with `account_type` |
| Balance | `GET /v2.0/account/balance/fin_num` | `balance_amt`, `available_amt`, `account_type`, `product_name`, `maturity_date` |
| Transaction history | `GET /v2.0/account/transaction_list/fin_num` | `inquiry_type`, `inquiry_base`, `from_date`, `to_date`, `sort_order`, `befor_inquiry_trace_info`; rows `tran_date`, `tran_time`, `inout_type` (입금/출금/지급/기타), `tran_type`, `print_content`, `tran_amt`, `after_balance_amt`, `branch_name` |
| Recipient inquiry | `POST /v2.0/inquiry/receive` | |
| Transfer | `POST /v2.0/transfer/withdraw/fin_num` | `bank_tran_id`, `fintech_use_num`, `tran_amt`, `wd_print_content`, `recv_client_bank_code`, `recv_client_account_num`, `recv_client_name`, `transfer_purpose` = `TR`; completes to the recipient in one call |
| Inbound deposit | `POST /v2.0/transfer/deposit/fin_num` | counterparty client, scope `oob`; generator listener only; `req_list` with one item |
| Transfer result | `POST /v2.0/transfer/result` | by original `bank_tran_id` and date |
| Payees (ext) | `GET/POST/DELETE /v2.0/ext/payees` | |
| Products (ext) | `GET /v2.0/ext/products` | |
| Term deposits (ext) | `POST /v2.0/ext/deposits`, `GET /v2.0/ext/deposits/{id}`, `POST /v2.0/ext/deposits/{id}/close` | |
| Loans (ext) | `POST /v2.0/ext/loans`, `GET /v2.0/ext/loans/{id}`, `POST /v2.0/ext/loans/{id}/repay` | |

Common response envelope: `api_tran_id` aNS(40), `api_tran_dtm` N(17), `rsp_code` AN(5), `rsp_message` AH(300), and for bank-level outcomes `bank_tran_id`, `bank_tran_date`, `bank_code_tran`, `bank_rsp_code` AN(3), `bank_rsp_message`; plus `request_id`. `AH(n)` lengths are validated as characters. Request and response schemas, and the response code table, are defined in `contracts/`.

### ATM interface (generator listener only)

`POST /atm/v1/session` (terminal ID, account number, PIN) → ATM session token; `POST /atm/v1/cash/withdraw`, `POST /atm/v1/cash/deposit` (idempotency key per terminal), `GET /atm/v1/balance`, `POST /atm/v1/session/end`.

### Admin interface (public listener, behind Cloudflare Access)

`POST /admin/v1/login`, then read-only endpoints backing F-39.

### Internal interface (generator listener only)

`POST /internal/v1/seed/{customers,accounts,atm_terminals,counterparty_clients,counterparties,analysts}`; `GET /internal/v1/sms/{phone}` (latest reset code); `GET /internal/v1/clock`; `POST /internal/v1/clock/set`, `/clock/advance` (backfill only); `POST /internal/v1/mode/live`.

### Simulated context headers (generator listener only)

`X-Request-Id` (ULID), `X-Sim-Ip`, `X-Sim-Region` (ISO 3166-2 code or a foreign country code), `X-Sim-Device` (device ID). On the public listener these headers are ignored, IP and region come from `CF-Connecting-IP` and `CF-IPCountry`, and the session is marked human.

### Future detection contract (not implemented in `product-init`)

`GET /v2.0/inquiry/fds_detect` following KFTC fields (`detect_time`, `policy_rule`, account, `tran_amt`, `recv_client_bank_code`, `recv_client_account_num`, `doubt_grade_code` A to D), served from an alert table written only by the detector. It never reads ground truth and never gates a transaction.

### Oracle logical data model

Korean logical names follow the MyData-based model; physical names are English. Surrogate numeric keys; natural identifiers are unique constraints. Business time columns have no default.

| Logical (physical) | Key attributes |
|---|---|
| 고객 (`CUSTOMER`) | `customer_id`; `customer_no` unique, `ci_hash`, name, phone unique, address, customer type |
| 고객인증 (`CUSTOMER_CREDENTIAL`) | customer; password hash, failed count, locked until, password changed at |
| 기기 (`DEVICE`) | customer, device ID, first seen, last seen |
| 인증세션 (`AUTH_SESSION`) | session ID, customer, device, listener, human flag, IP, region, started, last seen, ended, end reason |
| 로그인시도 (`LOGIN_ATTEMPT`) | phone, customer (nullable), outcome, device, IP, region, request ID, business time |
| 비밀번호재설정 (`PASSWORD_RESET`) | customer, code hash, issued, expires, used |
| 문자발송함 (`SMS_OUTBOX`) | phone, message, business time |
| 분석가 (`ANALYST`) | analyst ID, name, password hash |
| 기관 (`INSTITUTION`) | `institution_code` (bank_code_std), name, type |
| 이용기관 (`COUNTERPARTY_CLIENT`) | client ID, secret hash, institution code |
| 외부거래처 (`COUNTERPARTY`) | counterparty ID, type (employer, merchant, individual), industry code, institution code, account number, name |
| 상품 (`PRODUCT`), 상품조건버전 (`PRODUCT_TERMS`) | product type (demand, term, loan), terms version |
| 통합계좌 (`ACCOUNT`) | `account_id`; `account_no` unique, customer, product terms, subtype, status, opened at, last customer activity at, balance (held or owed), next sequence |
| 계좌상태이력 (`ACCOUNT_STATUS_HISTORY`) | account, from, to, reason, business time |
| ATM인증 (`ATM_CREDENTIAL`) | account, PIN hash, failed count, locked until |
| 수신계좌상세 (`DEPOSIT_DETAIL`) | term account; linked account, principal, maturity date, closed at, close type |
| 대출계좌상세 (`LOAN_DETAIL`) | loan account; linked account, principal, disbursed at, maturity date |
| 핀테크이용번호 (`FINTECH_ACCOUNT`) | `fintech_use_num` unique, account, `user_seq_no` |
| 토큰 (`ACCESS_TOKEN`) | token hash, subject (customer, client, analyst), scope, session, expires |
| 수취인 (`PAYEE`) | customer, bank code, account number, holder name, alias, registered at, channel, session, deleted at |
| 자동화기기 (`ATM_TERMINAL`) | terminal ID, region code, location |
| ATM세션 (`ATM_SESSION`) | ATM session ID, terminal, account, started, ended, request ID |
| 거래요청 (`TRANSACTION_REQUEST`) | `request_id` (generator or bank ULID); type, channel, listener, idempotency scope and key (unique), request hash, `bank_tran_id`, status, rsp_code, bank_rsp_code, session, device, terminal, IP, region, business time, system time |
| 이체 (`TRANSFER`) | transfer ID, kind, request, `bank_tran_id`, amount, debit row, credit row, business time |
| 수신거래/대출거래 (`ACCOUNT_TRANSACTION`) | `tran_id`; account, account sequence, signed amount, balance after, `inout_type`, `tran_type`, `print_content`, counterparty bank code, account number and name (nullable), channel, terminal, request ID, transfer, reversal of, business date and time, system time |
| 일계좌합계 (`DAILY_ACCOUNT_TOTAL`) | account, business date; transfer out, cash in, cash out |
| 일마감 (`DAILY_CLOSE`) | account, business date, opening balance, closing balance, verified, mismatch detail |
| 시뮬레이션시계 (`SIM_CLOCK`) | mode, business time, last advanced at |
| 이벤트발행함 (`OUTBOX`) | event ID, event JSON, created system time, delivered at |
| 공통코드 (`CODE`), 코드매핑 (`CODE_MAP`) | internal codes and their Open Banking and MyData mappings (`inout_type`, `tran_type`, `account_type`, response codes) |

Indexes support the access paths: transaction rows by (account, business date, account sequence); requests by idempotency scope and key and by request ID; login attempts by phone and time; sessions by customer; payees by customer; outbox by delivered state.

### Log envelope (MongoDB `bank_logs`, portable to PostgreSQL JSONB)

```json
{
  "schema": "saltation.log/1",
  "event_id": "01J...(ULID)",
  "event_type": "login.failed",
  "occurred_at": "2026-01-03T09:12:44.120+09:00",
  "recorded_at": "2026-11-12T03:00:01.004Z",
  "request_id": "01J...", "session_id": null, "device_id": "d-...",
  "customer_no": "C000012345", "account_no": null,
  "channel": "MOBILE", "listener": "generator",
  "ip": "203.0.113.10", "region": "KR-11",
  "outcome": "rejected", "rsp_code": "O0001", "bank_rsp_code": null,
  "human": false,
  "payload": {}
}
```

No ObjectId, BSON Date or Decimal128 in the envelope or payload; `_id` equals `event_id`. Bank-originated events have `request_id` null and `channel` `SYSTEM`.

### Truth store (separate MongoDB instance)

`labels`: `{request_id, subject_type, subject_id, label, scenario, scenario_instance_id, run_id}`, unique on (`request_id`, `subject_type`, `subject_id`); `subject_type` is `account` or `customer`; `label` is `normal`, `lookalike` or `A`..`F`. `runs`: run manifest with seed, population, amplification factor, scenario counts, code version and clock range.

### Dependencies

Rust (axum, tokio, the `oracle` crate over Oracle Instant Client for Linux arm64 and amd64, the MongoDB Rust driver, an Argon2 library, a Prometheus exporter), Oracle Database 19c EE (owner-built image), MongoDB, Node.js and pnpm, React and TypeScript, Python 3 with httpx and asyncio, nginx, Cloudflare Tunnel with Cloudflare Workers VPC and Cloudflare Access, Grafana Alloy, Loki, Prometheus, Grafana.

## Security, privacy, and policy constraints

- Synthetic data only; `ci_hash` hashes synthetic data. No resident registration numbers in any form.
- Passwords, PINs and reset codes are stored as Argon2id hashes; tokens and client secrets are stored hashed. None is logged.
- The truth store is a separate MongoDB instance on a network the bank is not attached to. The bank's MongoDB user can access only `bank_logs`; the generator has no network route to Oracle or `mongo`.
- The generator listener, ATM routes, inbound deposits, internal routes and metrics are never reachable through the tunnel; tunnel ingress allows only `web` and the public listener.
- Negative tests prove: the tunnel route cannot reach `/atm`, `/internal`, `/v2.0/transfer/deposit/fin_num` or `/metrics`; simulated context headers are ignored on the public listener; the bank container cannot reach `mongo-truth`; the generator container cannot reach `oracle` or `mongo`.
- Secrets come from an untracked `.env`; no secret is committed.
- Oracle binaries and the KFTC specification PDF are never committed. Oracle use is limited to development and demonstration of this personal portfolio project, subject to the owner's license check at download.

## Risks and concerns

- **Oracle 19c EE on ARM64:** build time, image size, memory on a shared host, no patches without support.
- **Oracle Rust driver on ARM64:** the `oracle` crate needs Instant Client for Linux ARM64.
- **Backfill throughput** over HTTP with Argon2 login hashing within 3 vCPU (N-4, N-6).
- **Partitioning license** uncertain (N-13).
- **Open Banking fidelity:** the KFTC portal and PDF differ on `inout_type` values; v3.3.6 (PDF) is pinned.
- **MyData code values** not yet verified against the official standard.
- **Scenario realism** relies on assumptions where no statistics exist (A, D, E, F); documented in generator config.

## Acceptance criteria

The `product-init` change is accepted when:

1. `docker compose up` on the development host starts every service, and all health checks report healthy within 10 minutes (N-11).
2. The generator seeds 20,000 customers, backfills 365 business days within 24 hours, switches to live mode and runs 24 hours with no manual intervention; N-8 holds throughout.
3. The run manifest shows every persona type active, the scenario matrix's yearly counts met in the backfill, its live minimums met in the live run, and the amplification factor.
4. For every scenario instance, the scenario matrix's required records exist in Oracle, and the corresponding events exist in `bank_logs`, including rejected attempts.
5. Every generator request ID with a label has, for each labelled subject, exactly one label, and every non-human request in Oracle has labels for all its subjects; B-to-C transfers carry B for the sender's account and C for the receiver's account; human sessions have no labels.
6. Daily close reports zero mismatches for every business date of the run, and N-5 holds.
7. Transfer load tests meet N-1, N-2 and N-3; their reports are stored as evidence.
8. In the mobile web app a person can sign in, reset the password, view balances and history, manage payees, transfer, open and early-close a term deposit, and take and repay a loan; in the admin web app an analyst can perform every F-39 lookup.
9. For backfilled records `occurred_at` lies within the backfill clock range and `recorded_at` within the run's wall-clock window; no business time column was set from wall-clock time.
10. No undelivered outbox event is older than 60 seconds at the end of the run (F-36).
11. Grafana shows `app.log` lines from Loki and bank metrics from Prometheus, including N-1 percentiles and outbox lag.
12. The negative tests in Security pass.
13. Saltation's disk use stays within N-9; all images build for arm64 and amd64 (Oracle from documented steps).

## Open questions

- MyData standard API document (bank sector): obtain the official version and pin `account_type`, transaction type and status codes in `contracts/codes`.
- Synthetic `bank_code_std` values for Saltation and for simulated external banks.
- Whether Oracle Partitioning is permitted by the license shown at download.
- Domain names for the web and API origins, and whether Saltation shares the host's existing `cloudflared` connector or runs its own.
