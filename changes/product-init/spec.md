# Specification

## Summary

This specification turns the approved intent into a buildable design for the **data foundation**: the Rust bank, its Open-Banking-shaped public API, the simulated ATM channel, the customer mobile web app, the analyst admin web app, the Python synthetic generator with ground truth, the Oracle ledger, the MongoDB logs, and the Docker Compose environment on the OCI development host.

Real-time detection is out of scope for the `product-init` change. This specification fixes only the hooks detection will need (a transactional outbox, correlation identifiers, an `fds_detect`-shaped alert contract) so that a later change can add detection without reshaping the data foundation.

References used for interface and data shapes:

- KFTC Open Banking user-institution API specification v3.3.6 (오픈뱅킹공동업무 이용기관 API명세서 v3.3.6, KFTC Open API portal, materials library). Paths, field names, types and lengths are followed; the document itself is not copied into the repository.
- The Korean financial MyData bank-sector data model, through the owner's analysis of a MyData-based target logical data model (customer, institution, product, integrated account, deposit account detail and transaction, loan account detail and transaction). Code-level values are pinned in a later revision once the official MyData standard API document is obtained (see Open questions).

## Requirements

### Functional

**Customers and authentication**

- F-1. The bank stores customers with a synthetic connecting-information hash (`ci_hash`) in place of a resident registration number, a name, a mobile phone number, an address and a persona-free customer type. Persona and fraud role are never stored in the bank.
- F-2. A customer signs in to the mobile web app with phone number and password (local login, simplified). Successful login issues a bearer access token with scopes `login inquiry transfer` and returns `user_seq_no`. Tokens expire; the expiry uses bank business time.
- F-3. The bank records every login attempt, success or failure, and every password reset, with device and session identifiers. Failed-login count and last password change time are kept on the customer's credential record.
- F-4. Devices are identified by a client-generated device identifier; the first login from an unseen device is recorded as a new-device event.

**Accounts and products**

- F-5. Products: demand deposit (입출금), term deposit (정기예금), loan (대출). Each product has an immutable terms version (term in months, minimum and maximum amounts, early-close allowed). No product carries an interest rate.
- F-6. Opening a demand deposit account assigns an account number and an Open Banking `fintech_use_num` (24 characters) bound to that account.
- F-7. Opening a term deposit debits a linked demand deposit account and credits the term deposit with the same principal. Early close and maturity return exactly the principal to the linked account. No interest is computed.
- F-8. Taking a loan credits the principal to a linked demand deposit account. Repayment debits the linked account and reduces the outstanding loan balance by exactly the repaid amount. No interest is computed. Repayment beyond the outstanding balance is rejected.
- F-9. All amounts are whole Korean won (`NUMBER(15,0)`); there is no currency other than KRW.
- F-10. An account has a status (active, dormant, closed) with a history of changes and a last-activity business time. Dormancy is derived from inactivity by a daily batch using business time.

**Transfers, payees and cash**

- F-11. A customer registers, lists and deletes payees (bank code, account number, holder name, alias). Registration time, channel and session are recorded.
- F-12. A transfer moves money from the customer's demand deposit account to an in-bank account or to a simulated external counterparty. A transfer to a payee that is not registered is allowed (as in real banks) and is recorded as such.
- F-13. A recipient inquiry (`/v2.0/inquiry/receive`) returns the holder name of the destination account before a transfer.
- F-14. Daily transfer limit per account (business date, KST). Exceeding it rejects the transfer with an Open Banking-style response code.
- F-15. The simulated ATM authenticates with account number and a 4-digit PIN, and performs cash deposit, cash withdrawal and balance inquiry against a registered ATM terminal (terminal identifier and location region).
- F-16. Every request that can move money carries an idempotency key; for Open Banking-shaped requests the key is `bank_tran_id`. A repeated request with the same key and identical payload returns the original result; with a different payload it is rejected.
- F-17. Rejected attempts (insufficient balance, limit exceeded, invalid account, wrong PIN, unauthorized) are recorded with their response codes and no ledger effect.

**Ledger (simple, single-entry)**

- F-18. Every money movement writes one account transaction row per in-bank account it affects, with a signed amount and the balance after. A transfer between two in-bank accounts writes two rows (debit and credit) linked by one transfer record. Cash, external-counterparty, term-deposit and loan movements write rows only for the in-bank accounts involved, with counterparty details where they exist. There are no internal ledger accounts and no double-entry journals.
- F-19. Account balances are kept on the account row and updated in the same database transaction as the transaction rows, under a row lock.
- F-20. A daily close (business date) records each account's closing balance and verifies that the opening balance plus the day's transaction rows equals the stored balance, and that every in-bank transfer's two rows have equal and opposite amounts.
- F-21. Corrections are new reversing rows referencing the original; written rows are never updated or deleted.

**Simulated clock**

- F-22. The bank reads business time only from its simulated clock. In live mode the clock follows wall-clock time. In backfill mode the clock is set and advanced only by the generator through the internal interface, forward only, and backfill mode can only be entered before live mode starts.
- F-23. Every business record carries business time; every record also carries system (wall-clock) recording time.

**Seeding and generator**

- F-24. The generator creates customers, accounts, ATM terminals and external counterparties through the internal seeding interface, not through customer channels.
- F-25. The generator drives all customer activity through the public mobile and ATM interfaces only, in backfill (about one year, accelerated simulated time) and then in live mode (wall-clock time).
- F-26. Personas: office worker, student, self-employed, elderly, each with a behaviour model (salary days, recurring payments, cash habits, active hours, device and location habits). Fraud actors play scenarios A to F of the intent; benign lookalikes play legitimate travel, new phone, one-off large purchase and shared household network.
- F-27. Scenario frequencies start from published statistics and are scaled by a configurable amplification factor recorded with each run. Default population 20,000 customers. Default yearly instance counts at amplification 1 and the default amplification are fixed in `generator/config` and documented with their sources.
- F-28. Ground truth: for every generated event, the generator writes one label per involved account (normal, benign lookalike, or primary scenario A to F with a scenario instance identifier) keyed by the event's opaque identifiers, to a truth store the bank cannot access. Human sessions are marked by the bank (session flag) and excluded from labelled datasets.

**Logs**

- F-29. The bank emits structured log events (login, logout, failed login, password reset, new device, session start and end, payee change, request outcome, ATM operation) to MongoDB through a transactional outbox, so a log event exists if and only if its banking transaction committed; rejected attempts are written directly.
- F-30. Every log event uses one versioned JSON envelope (see Interfaces) with string identifiers and ISO-8601 timestamps only.

**Mobile web app (customer)**

- F-31. Sign in, view accounts and balances, view transaction history with pagination, manage payees, recipient inquiry and transfer, open and early-close a term deposit, take and repay a loan.

**Admin web app (analyst)**

- F-32. Analyst sign-in (separate analyst accounts). Search customers by name, phone or customer number; view a customer's accounts, balances, payees, devices and recent sessions; view an account's transactions with filters (date range, channel, direction, amount); view a transfer's both legs; view daily close results.

### Non-functional

- N-1. **Transfer latency**, server-side, measured from request received to response sent, on the development host, at a sustained 50 transfers per second for 10 minutes: p50 ≤ 50 ms, p95 ≤ 300 ms, p99 ≤ 1,000 ms; row-lock wait p95 ≤ 50 ms.
- N-2. **Hot account:** many concurrent transfers into one account (mule pattern): p95 ≤ 500 ms.
- N-3. **Saturation point:** under a stepped load (25, 50, 100, 200 transfers per second), the load at which p95 first exceeds 300 ms is at least 100 per second; the limiting resource (lock wait, disk, CPU, connection pool) is recorded.
- N-4. **Backfill throughput:** at least 200 generated banking operations per second sustained, so one year for 20,000 customers completes within 24 hours.
- N-5. **Correctness:** zero daily-close mismatches, zero duplicate money movements, and zero in-bank transfers whose two rows do not offset, under all load tests.
- N-6. **Resource budget for Saltation** on the shared host: at most 3 vCPU total CPU limits and 14 GB total memory limits across Saltation containers; at most 70 GB of the block volume for one year of data plus 30 days of live data and observability retention. Initial limits: Oracle 6 GB (SGA 4 GB, PGA 1 GB), MongoDB 3 GB (WiredTiger cache 1.5 GB), bank server 1 GB, generator 1 GB, nginx 256 MB, Alloy 256 MB, Loki 768 MB, Prometheus 768 MB, Grafana 256 MB. During backfill and the 24-hour run no Saltation container is OOM-killed or restarted, and the co-tenant containers stay healthy.
- N-7. **Portability:** every image is published for, or (Oracle) buildable from documented steps on, linux/arm64 and linux/amd64.
- N-8. **Operability:** one `docker compose up` command starts all server-side services after documented prerequisites (the Oracle image built locally, the Cloudflare tunnel token in `.env`). The mobile web app needs no installation; it is served by the stack.
- N-10. **Observability:** the bank server exposes Prometheus metrics (request rate, latency histograms per endpoint, transfer outcomes, row-lock wait, DB pool usage) and writes structured operational logs as JSON lines to `app.log` on a log volume, rotated by size and age; Grafana dashboards show transfer latency percentiles (N-1), error rates and container resource use.
- N-9. **Partitioning is optional.** The design performs within N-1 to N-5 without Oracle Partitioning. Tables carry business date so that monthly interval partitioning can be enabled later if the license permits it.

## User experience and behavior

- **Mobile web (customer, Korean UI):** login screen (phone, password); home with account cards and balances; account detail with transaction list (date, time, description `print_content`, amount, balance after); transfer flow: choose payee or enter bank and account → recipient inquiry shows holder name → amount → confirm → result with response code message; payees screen; products screen to open term deposits and take loans; loan screen with outstanding balance and repay.
- **Admin web (analyst, Korean UI):** login; customer search; customer 360 view (profile, accounts, payees, devices, recent sessions and failed logins); account transaction table with filters and CSV export; transfer detail with both legs and request outcome; daily close report.
- **Error behaviour:** every API response carries `rsp_code` and `rsp_message`; bank-level outcomes also carry `bank_rsp_code` and `bank_rsp_message`. Messages are Korean.
- **Time display:** all times shown in KST; business time is what customers and analysts see.

## Architecture and design

### Top-level modules and ownership

The code is organised **by feature** (vertical slices), not by technical layer: the Rust server follows vertical-slice / package-by-feature, the React apps follow the Bulletproof React layout, and the generator groups code by persona and scenario.

| Module | Path | Owns | Language |
|---|---|---|---|
| Contracts | `contracts/` | OpenAPI documents for the public, ATM, admin and internal interfaces; the log envelope JSON Schema; code tables | YAML, JSON |
| Bank server | `bank/` | The Rust API server, one feature folder per banking capability (below) | Rust, SQL |
| Web | `web/apps/mobile`, `web/apps/admin`, `web/packages/api-client` | Customer and analyst SPAs; a typed client generated from `contracts/` | TypeScript, React |
| Generator | `generator/` | Personas, scenario engine, backfill and live drivers, seeding, truth writer, run manifests | Python |
| Infra | `infra/compose`, `infra/oracle`, `infra/nginx`, `infra/observability`, `infra/lab` | Compose files, Oracle image build steps and init scripts, nginx config, Alloy/Loki/Prometheus/Grafana config and dashboards, later Terraform/Ansible for the VMware lab | YAML, shell, HCL |
| Load tests | `loadtest/` | Transfer latency, hot-account and saturation tests (N-1 to N-5) | Python |

**Bank server layout**

```text
bank/
├── migrations/            # numbered plain-SQL schema migrations
└── src/
    ├── main.rs            # config, wiring, startup
    ├── app.rs             # composes feature routers and middleware
    ├── shared/            # DB pool and transaction helper, simulated clock, auth extractor,
    │                      # error and response envelope, money and identifier types,
    │                      # outbox writer, app.log and metrics
    └── features/
        ├── auth/          # login, logout, password reset, devices, sessions
        ├── accounts/      # user info, account list, balance, transaction history
        ├── transfers/     # recipient inquiry, transfer, transfer result, daily limits
        ├── payees/
        ├── deposits/      # term deposits
        ├── loans/
        ├── atm/
        ├── admin/         # analyst queries
        ├── internal/      # seeding and clock (internal network only)
        ├── daily_close/   # daily close batch
        └── outbox/        # relay of business events to MongoDB
```

Each feature folder holds its router (`mod.rs`), handlers, domain rules, repository and its own `sql/` directory of hand-written Oracle statements.

**Web app layout** (both apps): `src/app/` (router, providers, shell) → `src/features/<feature>/` (`api/`, `components/`, `hooks/`, `types.ts`, `index.ts`) → `src/shared/` (UI kit, formatters). Mobile features: auth, accounts, transfers, payees, deposits, loans. Admin features: auth, customer-lookup, account-transactions, transfer-detail, daily-close.

**Generator layout:** `generator/saltation_gen/` with `personas/`, `scenarios/` (one module per scenario A to F and per benign lookalike), `drivers/` (seed, backfill, live), `client/` (generated from `contracts/`), `truth/`, `config/`.

### Dependency direction

- Inside each app: `app` → `features` → `shared`, one way only. A feature never imports another feature's internals; when two features need the same thing it moves to `shared`. In Rust, feature internals are `pub(crate)` only through the feature's `mod.rs`; in TypeScript, other code imports a feature only through its `index.ts`.
- `shared` never imports from `features`.
- `web` and `generator` depend only on `contracts/` (generated or validated clients), never on bank internals or databases.
- The generator has no access to Oracle or the bank's MongoDB database; the bank has no access to the truth store.
- **Where new features go:** a new banking capability is a new folder under `bank/src/features/`, with its interface first added to `contracts/`; a new customer or analyst screen is a new folder under the app's `src/features/`. Detection, when added, is a new top-level module (`detect/`) consuming the outbox stream; it does not import bank code. A later Python data service (pandas and NumPy analysis over Oracle and MongoDB data) is a new top-level module (`analytics/`) that reads data stores read-only.
- Boundary enforcement: an ESLint `import/no-restricted-paths` rule in both web apps, an import-boundary check for `bank/src` (no `features::<other>::` paths, no `features` paths from `shared`), and an import lint for the generator. The plan registers each as a check.
- A project verification skill (`verify-saltation`) keeps one feature file per user-facing feature, matching the feature folders (see the `verification-map` skill).

### Runtime topology (Docker Compose, one network per trust zone)

```text
browser ─HTTPS─▶ Cloudflare (TLS, Workers VPC) ─tunnel─▶ nginx (static SPAs only: mobile web, admin web)
browser ─HTTPS─▶ Cloudflare (TLS, Workers VPC) ─tunnel─▶ Rust bank API server (standalone API)
                                                          ├─▶ Oracle Database (structured data)
                                                          ├─▶ MongoDB (unstructured data and business event logs)
                                                          ├─ app.log ─▶ Alloy ─▶ Loki ─▶ Grafana
                                                          └─ /metrics ─▶ Prometheus ─▶ Grafana
generator ─HTTP (private network)─▶ Rust bank API server (public, ATM, internal interfaces)
```

| Service | Image | Networks |
|---|---|---|
| `oracle` | locally built Oracle Database 19c EE | `data` |
| `mongo` | official MongoDB (arm64 and amd64) | `data`, `truth` |
| `bank` | built from `bank/`; serves the API over HTTP on the private network; writes `app.log` to the `applogs` volume | `data`, `edge`, `internal`, `obs` |
| `web` | nginx serving the two SPAs over HTTP on the private network; no API proxying | `edge` |
| `cloudflared` | Cloudflare Tunnel connector used by Workers VPC to reach `web` and `bank` | `edge` |
| `generator` | built from `generator/` | `edge` (public and ATM interfaces), `internal` (seeding and clock), `truth` |
| `alloy` | Grafana Alloy, tailing `app.log` from the `applogs` volume (read-only mount) | `obs` |
| `loki` | Grafana Loki | `obs` |
| `prometheus` | Prometheus, scraping `bank` metrics | `obs` |
| `grafana` | Grafana with Loki and Prometheus data sources | `obs` |

- Two log streams with different purposes: **business event logs** (login, session, device, payee, ATM and request outcomes) go to MongoDB through the outbox and are product data for detection and analysis; **operational logs** (request traces, errors, timings) are written to `app.log` and shipped by Alloy into Loki for monitoring. Both carry the same `request_id` and `session_id`.
- The SPAs call the API on its own origin, so the bank server enforces a CORS allow-list of the two SPA origins.

- The truth store is a separate MongoDB database (`truth`) with its own user; the bank's MongoDB user is granted only the `bank_logs` database. The plan may move truth to a separate MongoDB instance if credential separation proves insufficient.
- The internal interface (seeding, clock) listens only on the `internal` network and is disabled in live mode except for read-only clock status.
- Local testing uses plain HTTP: a `local` Compose profile publishes `web` and `bank` on localhost ports and omits `cloudflared`; the SPAs read the API base URL from build-time configuration, and the CORS allow-list includes the local origins only in that profile.
- TLS is terminated by Cloudflare. Public traffic reaches `web` and `bank` only through Cloudflare Workers VPC over the `cloudflared` tunnel; no Saltation port is published on the host. Grafana is reached through the tunnel behind Cloudflare Access, or through an SSH tunnel. Oracle, MongoDB, Loki and Prometheus are never exposed.

### Data access: plain Oracle SQL, no ORM

- No ORM and no query builder. Every statement is hand-written Oracle SQL in its own file under the owning feature's `sql/` directory (for example `bank/src/features/transfers/sql/lock_accounts.sql`, `insert_transaction.sql`), embedded at compile time and executed through the `oracle` crate with bind variables only; no SQL is assembled by string concatenation.
- Each SQL file starts with a comment naming the statement, its purpose and its expected access path (index), so the exact query the bank runs can be read in the repository and checked with `EXPLAIN PLAN`.
- Schema changes are numbered plain SQL migration files under `bank/migrations/`, applied in order and recorded in a migration history table.
- The operational log (`app.log`) records, for every statement, its name, duration and row count; at debug level it also records the SQL text. Bind values are never logged for credential columns (password, PIN, token hashes).

### Transfer processing (in one Oracle transaction)

1. Insert the request into `TRANSACTION_REQUEST` with the idempotency key; a unique-constraint violation returns the stored result.
2. Lock the in-bank accounts involved with `SELECT ... FOR UPDATE` in ascending account-ID order (deadlock avoidance).
3. Check status, balance and the daily limit (`DAILY_ACCOUNT_TOTAL` row for account and business date, locked with the account).
4. Insert the transfer record and its account transaction rows; update the balances and the daily totals; insert outbox rows.
5. Update the request with the outcome; commit. On a business rejection, record the outcome with no transaction rows and commit.

## Interfaces, data, and dependencies

### Public interface (Open Banking-shaped, used by the mobile web app and the generator)

Paths, field names, types and lengths follow KFTC v3.3.6 where an equivalent exists. Saltation acts as hub and provider bank at once, with a synthetic bank code (`bank_code_std`) chosen from an unassigned value and recorded in `contracts/codes`.

| Function | Method and path | Notes |
|---|---|---|
| Login (extension) | `POST /v2.0/ext/auth/login` | phone, password, device ID → `access_token`, `expires_in`, `scope`, `user_seq_no` |
| Logout | `POST /oauth/2.0/revoke` | |
| User info | `GET /v2.0/user/me` | accounts with `fintech_use_num`, `account_num_masked` |
| Account list | `GET /v2.0/account/list` | |
| Balance | `GET /v2.0/account/balance/fin_num` | `balance_amt`, `available_amt`, `account_type`, `product_name` |
| Transaction history | `GET /v2.0/account/transaction_list/fin_num` | `inquiry_type`, `inquiry_base`, `from_date`, `to_date`, `sort_order`, `befor_inquiry_trace_info`; rows `tran_date`, `tran_time`, `inout_type` (입금/출금/지급/기타), `tran_type`, `print_content`, `tran_amt`, `after_balance_amt`, `branch_name` |
| Recipient inquiry | `POST /v2.0/inquiry/receive` | |
| Transfer | `POST /v2.0/transfer/withdraw/fin_num` | `bank_tran_id`, `fintech_use_num`, `tran_amt`, `wd_print_content`, `recv_client_bank_code`, `recv_client_account_num`, `recv_client_name`, `transfer_purpose` = `TR`; Saltation completes the transfer to the recipient in the same call (the KFTC two-step via a user institution account is collapsed) |
| Transfer result | `POST /v2.0/transfer/result` | lookup by original `bank_tran_id` and date |
| Payees (extension) | `GET/POST/DELETE /v2.0/ext/payees` | |
| Term deposits (extension) | `POST /v2.0/ext/deposits`, `POST /v2.0/ext/deposits/{id}/close` | |
| Loans (extension) | `POST /v2.0/ext/loans`, `POST /v2.0/ext/loans/{id}/repay` | |
| Password reset (extension) | `POST /v2.0/ext/auth/password_reset` | simplified: phone plus a generator-supplied verification code |

Common response envelope: `api_tran_id` aNS(40), `api_tran_dtm` N(17), `rsp_code` AN(5), `rsp_message` AH(300), and for bank-level outcomes `bank_tran_id`, `bank_tran_date`, `bank_code_tran`, `bank_rsp_code` AN(3), `bank_rsp_message`. `AH(n)` lengths are validated as characters in UTF-8, not bytes.

### ATM interface (generator only)

`POST /atm/v1/session` (terminal ID, account number, PIN) → short-lived ATM session; `POST /atm/v1/cash/withdraw`, `POST /atm/v1/cash/deposit`, `GET /atm/v1/balance`. Each money call carries an idempotency key.

### Admin interface

`/admin/v1/...` read-only endpoints backing F-32, plus analyst login. Analysts cannot move money.

### Internal interface (generator only, `internal` network)

`POST /internal/v1/seed/customers`, `/accounts`, `/atm_terminals`, `/counterparties`; `POST /internal/v1/clock/set`, `/clock/advance` (backfill mode only, forward only); `POST /internal/v1/mode/live`; `GET /internal/v1/clock`.

### Future detection contract (not implemented in `product-init`)

`GET /v2.0/inquiry/fds_detect` following KFTC fields (`detect_time`, `policy_rule`, account, `tran_amt`, `recv_client_bank_code`, `recv_client_account_num`, `doubt_grade_code` A to D), served from an alert table written only by the detector. It never reads ground truth and never gates a transaction.

### Oracle logical data model

Korean logical names follow the MyData-based model; physical names are English. Surrogate numeric keys; natural identifiers are unique constraints. Business time columns are never defaulted from `SYSDATE`.

| Logical (physical) | Key attributes |
|---|---|
| 고객 (`CUSTOMER`) | `customer_id`; `customer_no` unique, `ci_hash`, name, phone unique, address, human_flag |
| 고객인증 (`CUSTOMER_CREDENTIAL`) | `customer_id`; password hash, failed_login_count, password_changed_at |
| 기관 (`INSTITUTION`) | `institution_code` (bank_code_std); name, type |
| 외부거래처 (`COUNTERPARTY`) | `counterparty_id`; type (employer, merchant, individual), industry code, institution_code, account number, name |
| 상품 (`PRODUCT`), 상품조건버전 (`PRODUCT_TERMS`) | product type (demand, term, loan), terms version |
| 통합계좌 (`ACCOUNT`) | `account_id`; `account_no` unique, customer_id, product terms, subtype (demand, term, loan), status, opened_at, last_activity_at, balance (customer accounts), PIN hash (demand) |
| 계좌상태이력 (`ACCOUNT_STATUS_HISTORY`) | account, status, business time |
| 수신계좌상세 (`DEPOSIT_DETAIL`) | account; linked account, principal, maturity date, closed_at, close type (maturity, early) |
| 대출계좌상세 (`LOAN_DETAIL`) | account; linked account, principal, outstanding, disbursed_at, maturity date |
| 핀테크이용번호 (`FINTECH_ACCOUNT`) | `fintech_use_num` unique, account, user_seq_no |
| 토큰 (`ACCESS_TOKEN`) | token hash, user_seq_no, scope, expires_at, device_id, session_id |
| 수취인 (`PAYEE`) | customer, bank code, account number, holder name, alias, registered_at, channel, session_id, deleted_at |
| 자동화기기 (`ATM_TERMINAL`) | terminal id, region code, location |
| 거래요청 (`TRANSACTION_REQUEST`) | `request_id`; type, channel, idempotency key and scope (unique), request hash, `bank_tran_id`, status, rsp_code, bank_rsp_code, event_id, session_id, device_id, terminal_id, business time, system time |
| 이체 (`TRANSFER`) | `transfer_id`; request_id, `bank_tran_id`, debit transaction row, credit transaction row (nullable for external), amount, business time |
| 수신거래/대출거래 (`ACCOUNT_TRANSACTION`) | `tran_id`; account, signed amount, balance_after, account sequence, `inout_type`, `tran_type`, `print_content`, counterparty bank code, account number and name (nullable), channel, terminal_id, request_id, transfer_id (nullable), event_id, reversal_of, business date and time, system time |
| 일계좌합계 (`DAILY_ACCOUNT_TOTAL`) | account, business date; transfer total, cash in, cash out |
| 일마감 (`DAILY_CLOSE`) | account, business date, opening balance, closing balance, verified flag |
| 이벤트발행함 (`OUTBOX`) | outbox id, event JSON, created system time, published flag |
| 공통코드 (`CODE`), 코드매핑 (`CODE_MAP`) | internal codes and their Open Banking and MyData mappings (`inout_type`, `tran_type`, `account_type`, response codes) |

Indexes support the access paths: transaction rows by (account, business date, account sequence); requests by idempotency key; payees by customer; accounts by customer.

### Log envelope (MongoDB `bank_logs`, portable to PostgreSQL JSONB)

```json
{
  "schema": "saltation.log/1",
  "event_id": "01J...(ULID)",
  "event_type": "login.failed",
  "occurred_at": "2026-01-03T09:12:44.120+09:00",
  "recorded_at": "2026-11-12T03:00:01.004Z",
  "customer_no": "C000012345",
  "account_no": null,
  "session_id": "...", "device_id": "...", "request_id": "...",
  "channel": "MOBILE",
  "ip": "203.0.113.10", "geo_region": "KR-11",
  "outcome": "rejected", "rsp_code": "O0001",
  "human": false,
  "payload": {}
}
```

No ObjectId, BSON Date or Decimal128 in the envelope or payload; MongoDB's `_id` is set to `event_id`.

### Truth store (MongoDB `truth`)

`labels`: `{event_id, account_no, side, label, scenario, scenario_instance_id, run_id}` where `label` is `normal`, `lookalike` or `A`..`F`. `runs`: run manifest with seed, population, amplification factor, scenario counts, code version and clock range.

### Dependencies

Rust (axum, tokio, the `oracle` crate over Oracle Instant Client for Linux arm64 and amd64, the MongoDB Rust driver, a Prometheus metrics exporter), Oracle Database 19c EE (owner-built image), MongoDB, Node.js and pnpm for the web build, React and TypeScript, Python 3 with httpx and asyncio for the generator, nginx, Cloudflare Tunnel (`cloudflared`) with Cloudflare Workers VPC, Grafana Alloy, Loki, Prometheus, Grafana.

## Security, privacy, and policy constraints

- Synthetic data only; `ci_hash` is a hash of synthetic data. No resident registration numbers in any form.
- Passwords and PINs are stored with a slow password hash; access tokens are stored hashed.
- Ground truth is unreachable from the bank (credentials and networks, see Architecture).
- Internal interfaces are reachable only on the internal Compose network and are never routed through the tunnel. Public access is HTTPS through Cloudflare only; Grafana is behind Cloudflare Access or an SSH tunnel.
- Secrets come from an untracked `.env` file; no secret is committed.
- Oracle binaries and the KFTC specification PDF are never committed. Oracle use is limited to development and demonstration of this personal portfolio project, subject to the owner's license check at download.

## Risks and concerns

- **Oracle 19c EE on ARM64:** image build time and size, memory sizing on a shared host, no patches without a support contract. Mitigation: early build-and-measure step in the plan.
- **Oracle Rust driver on ARM64:** the `oracle` crate needs Instant Client for Linux ARM64. Mitigation: prove a connection in the first plan step.
- **Backfill throughput** through HTTP with password hashing on login can fall below N-4. Mitigation: sessions are reused within a simulated day; measure early.
- **Partitioning license** uncertain. Mitigation: N-9, design without it.
- **Open Banking shape fidelity:** the KFTC public portal and the PDF differ on `inout_type` values; v3.3.6 (PDF) is pinned.
- **MyData code values** are not yet verified against the official standard.
- **Scenario realism** depends on assumptions where no statistics exist (A, D, E, F); documented in the generator config.

## Acceptance criteria

The `product-init` change is accepted when:

1. `docker compose up` on the development host starts all services healthy after the documented prerequisites.
2. The generator seeds 20,000 customers, backfills about one year of activity within 24 hours, switches to live mode, and runs 24 hours without manual intervention, OOM kill or restart, with co-tenant containers healthy (N-4, N-6).
3. In that run, every persona type and every scenario A to F appear at least the configured minimum count, and benign lookalikes appear; the run manifest records the amplification factor.
4. For every scenario instance, the spec-defined expected records exist in Oracle and in `bank_logs`, including rejected attempts.
5. Every generated event joins to exactly one label per involved account through opaque identifiers; B-to-C transfers carry B on the sender side and C on the receiver side; human sessions are excluded.
6. Daily close reports zero mismatches for every day of the run; every in-bank transfer's two rows offset; no duplicate money movement exists (N-5).
7. Transfer load tests meet N-1, N-2 and N-3, and their reports are stored as evidence.
11. Grafana shows the bank's operational logs from Loki and its metrics from Prometheus, including transfer latency percentiles.
8. A human can, in the mobile web app, sign in, view balances and history, manage payees, transfer, open and early-close a term deposit, and take and repay a loan; in the admin web app, an analyst can perform every F-32 lookup.
9. Business time and system time differ as expected for backfilled records; no business column was set from wall-clock time.
10. Disk use for the data stays within N-6; all images build for arm64 and amd64 (Oracle from documented steps).

## Open questions

- MyData standard API document (bank sector): obtain the official version and pin `account_type`, transaction type and status codes in `contracts/codes`.
- Synthetic `bank_code_std` value for Saltation and for simulated external banks.
- Daily transfer limit default and ATM withdrawal limit default.
- Default yearly scenario instance counts and amplification factor, with sources (starting point: the statistics gathered during the intent review).
- Whether Oracle Partitioning is permitted by the license shown at download.
- Whether the truth store needs a separate MongoDB instance rather than a separate database.
- Domain names for the web and API origins, and whether Saltation shares the host's existing `cloudflared` connector or runs its own.
