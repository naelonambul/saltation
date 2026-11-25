# Change Plan: `session-reuse-and-maintenance-window`

## Goal

Implement the approved `spec.md` changes:
1. **F-27a Daily Settlement Maintenance Window (`00:00:00`–`00:29:59.999999` KST, configurable via `SALTATION_MAINTENANCE_MINUTES`, default `30`, `0` disables):**
   - Reject the 9 monetary/ledger operations (single source of truth: `outcome::money_operation`) before handler execution with HTTP `503`, `rsp_code: A0013`, Korean message `"일일 정산 점검 중입니다"`, and `Retry-After` header (seconds remaining until window end).
   - Record `request.completed` in `OUTBOX` (F-35) with `request_type` populated, without claiming `bank_tran_id`/`idempotency_key` or inserting `TRANSACTION_REQUEST`/`CARD_AUTHORIZATION`/`card.declined` (F-18/F-19/F-43 exemption).
   - Keep all non-monetary routes (auth, password reset, read-only inquiries, payees, ATM session start/end, `/admin/*`, `/internal/*`, `/healthz`, `/readyz`, `/metrics`) and in-process F-27 rollover jobs open.
   - Prevent automated client hot-retry loops on `503` responses carrying `Retry-After`, guard `LIVE` cutover scheduling and `loadtest` preflight against the maintenance window, and disable the window in clock-independent test fixtures (`AppState.maintenance_minutes = 0` in test builders and `SALTATION_MAINTENANCE_MINUTES=0` in `it-oracle.sh` for wall-clock `LIVE` suites) while adding dedicated F-27a boundary tests.
2. **F-32 Normal Mobile Session Reuse & N-4/N-6 Alignment:**
   - Combine most normal balance+transfer visits into a deterministic `normal_visit` episode (`login` `+0s`, `balance` `+10s`, `transfer` `+60s`) inside the existing `0 mod 3` transfer lane using one F-2 30-minute session and device/region context, while keeping a deterministic ~25% share of standalone `normal_transfer` (`login` → `transfer`) visits so the presence of a prior balance inquiry never leaks the ground-truth label.
   - Adjust standalone `normal_login_balance` annual cadence accordingly so total annual requests per customer land in the approved ~415–435 range (~8.3M–8.7M/year for 20,000 customers) while preserving all fraud scenarios (A–G) and benign lookalikes with fresh logins.
3. **Baseline Promotion:**
   - Replace root `spec.md` with the approved `changes/session-reuse-and-maintenance-window/spec.md`.

## Order of Work

1. **Bank F-27a Maintenance Window (`bank/src/app.rs`, `bank/src/main.rs`, `bank/src/shared/context/mod.rs`, `bank/src/shared/outcome/mod.rs`, `bank/tests/{common/mod.rs,backfill.rs,listeners.rs,driver_bench.rs}`, `infra/compose/{compose.yml,.env.example}`, `scripts/dev/it-oracle.sh`):**
   - Add `pub maintenance_minutes: u32` to `AppState` (`main.rs` reads `SALTATION_MAINTENANCE_MINUTES`, default `30`, valid `0..=60`; test builders default to `0`).
   - Make `outcome::money_operation` `pub(crate)` and extend `Recorder::finish` so `503` on a `money_operation` sets `event.channel` and `payload.request_type` without calling `reject_attempt` (no `TRANSACTION_REQUEST` row).
   - In `context::operation`, after `state.clock.enter()` binds `BusinessTime(now)` and before `next.run(req)`, check `maintenance_retry_after(now, state.maintenance_minutes)` for requests matching `outcome::money_operation(&method, &matched_route)`. When active, return `503 A0013` (`"일일 정산 점검 중입니다"`) with `Retry-After: <secs>` through the existing `rejection` path so `Recorder::finish` records `request.completed` in `OUTBOX`.
   - Add unit tests in `context/mod.rs` covering `23:59:59.999999`, `00:00:00`, `00:29:59.999999`, `00:30:00` KST and `maintenance_minutes = 0`.
   - Add an Oracle integration test in `bank/tests/backfill.rs` proving: `00:00:00` and `00:29:59` reject `/v2.0/transfer/withdraw/fin_num` and `/card/v1/authorize` with `503 A0013` + `Retry-After`, write `request.completed` to `OUTBOX`, leave zero `TRANSACTION_REQUEST`/`CARD_AUTHORIZATION` rows, allow `/v2.0/account/balance/fin_num` and `/admin/v1/customers` during `00:15:00`, and allow the exact same `bank_tran_id`/card key to succeed at `00:30:00`.
   - Document `SALTATION_MAINTENANCE_MINUTES` in `infra/compose/compose.yml` and `infra/compose/.env.example`, and pass `SALTATION_MAINTENANCE_MINUTES=0` in `scripts/dev/it-oracle.sh` for wall-clock `LIVE` CI suites.
   - Proof: `cd bank && cargo fmt --check && cargo clippy --lib --bins --tests -- -D warnings && cargo test --lib`.

2. **Generator Client `Retry-After` & F-32 Normal Session Reuse (`generator/saltation_gen/client/bank.py`, `personas/profiles.py`, `drivers/stage6_steps.py`, `drivers/stage6_schedule.py`, `drivers/stage6.py`, `generator/tests/{test_stage6_schedule.py,test_stage6_steps.py,test_card_wire.py}`):**
   - In `BankClient._call`, do NOT retry a `503` response that carries a `Retry-After` header (prevents hot-retry loops and duplicate `request.completed` events during maintenance). Add unit test in `test_card_wire.py`.
   - In `profiles.py`, `stage6_steps.py`, and `stage6_schedule.py`:
     - Add `normal_visit` (`login` `+0s`, `balance` `+10s`, `transfer` `+60s`, 3 requests/episode) with a deterministic 75%/25% split (`normal_visit` vs standalone `normal_transfer`) so ~75% of normal transfers reuse the balance-inquiry session and ~25% remain standalone `login -> transfer` (no FDS label leak).
     - Reduce standalone `normal_login_balance` annual cadence by the combined `normal_visit` count so total balance inquiries and total transfers per customer stay identical to before while removing redundant logins (~8.5M requests/year, ~425/customer).
     - Guard `stage6_schedule.py` / `stage6.py` against scheduling `LIVE` monetary operations inside `[00:00, 00:30)` KST when `SALTATION_MAINTENANCE_MINUTES > 0`.
   - Update unit tests in `test_stage6_schedule.py` and `test_stage6_steps.py` to assert the ~8.3M–8.7M annual volume, 75/25 `normal_visit`/`normal_transfer` mix, and maintenance-window scheduling guard.
   - Proof: `generator/.venv/bin/ruff check generator && generator/.venv/bin/pytest generator/tests`.

3. **Loadtest Maintenance Preflight (`loadtest/saltation_loadtest/cli.py`, `loadtest/tests/test_cli.py`):**
   - Add a preflight check on `/internal/v1/clock` in `loadtest` so a run that would execute inside or cross `[00:00, 00:30)` KST (when `SALTATION_MAINTENANCE_MINUTES > 0`) reports a clear preflight warning/block instead of failing N-1/N-2 with spurious `503`s.
   - Proof: `loadtest/.venv/bin/ruff check loadtest && loadtest/.venv/bin/pytest loadtest/tests` (or repo `ci-loadtest.sh`).

4. **Promote Root `spec.md` & Full Verification:**
   - Copy `changes/session-reuse-and-maintenance-window/spec.md` to root `spec.md`.
   - Run `python3 scripts/repo.py status --change session-reuse-and-maintenance-window`, `python3 scripts/repo.py verify --change session-reuse-and-maintenance-window --group core`, and Rust/Python/Loadtest checks locally, then open PR for full CI including `repository/oracle`.

## Risks & Rollback

- **Risk:** Wall-clock `LIVE` CI jobs running at `00:00–00:30` KST (`15:00–15:30` UTC). **Mitigation:** `AppState.maintenance_minutes` defaults to `0` in unit/integration test builders and `it-oracle.sh` wall-clock fixtures, while `main.rs` and Compose default to `30`, plus dedicated F-27a boundary tests in `backfill.rs`.
- **Risk:** `normal_visit` step count or episode kind breaking `generator_lifecycle.rs`. **Mitigation:** `normal_visit` starts with `normal_`, which `generator_lifecycle.rs:644` and `prune_completed_normal_day` already handle generically; `normal_channels` still includes `card`, `atm_withdraw`, `inbound`.
- **Rollback:** Revert the single squash commit on `main` via a new `behavior` packet restoring `spec.md` and code.
