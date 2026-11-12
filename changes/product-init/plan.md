# Implementation Plan

## Summary

This plan delivers **stage 1** of the spec's Delivery table:

1. Establish the approved `intent.md` and `spec.md` as the root baseline.
2. Build a **walking skeleton** that proves the architecture end to end on the OCI development host. A customer is seeded, logs in, reads a balance and history, and makes an in-bank transfer from the mobile web app or the generator. The transfer is written to Oracle Database 19c EE with locking and idempotency. Its business events reach MongoDB through the outbox. Its operational log reaches Grafana through Alloy and Loki, and its metrics through Prometheus.
3. Register the stack's quality checks, including an Oracle integration group that runs on a self-hosted runner on the OCI host.

Stages 2 to 6 (ledger and products, ATM, admin web, generator scenarios, operations) are separate changes with their own plans.

Development for this stage happens on the owner's Mac (Apple Silicon, arm64, the same CPU architecture as the OCI host) in `/Users/ckh/repos/saltation`, with Docker provided by Colima (VM sized 6 CPU, 10 GB memory, 100 GB disk). Commits, pushes and pull requests use the Mac's `agent-git` and `agent-gh` wrappers. Toolchains (Rust, Node, Python packages) run in containers, so the Mac needs only Docker, Git and Python 3. The OCI host runs the self-hosted CI runner for the Oracle integration group; the Oracle image built on the Mac is transferred to it with `docker save | ssh oracle docker load`. Running the stack on the OCI host is stage 6.

### Spec acceptance criteria proved by this change

The stage-1 subset, at skeleton scale (1,000 seeded customers, no backfill):

- **AC 1:** the stage-1 services start healthy.
- **AC 6:** zero duplicate money movements and offsetting transfer rows. Daily close itself arrives in stage 2.
- **AC 10:** the outbox drains, including the MongoDB-stop test.
- **AC 11:** Grafana shows `app.log` lines and bank metrics.
- **AC 12:** the negative tests that apply to the skeleton:
  - The public listener ignores simulated-context headers.
  - `/internal` is absent from the public listener.
  - The bank's MongoDB user is denied on `truth`.

Every other criterion is proved by later stages.

## Prerequisites (owner)

- **P1. Oracle installation file.** Done: `LINUX.ARM64_1919000_db_home.zip` is in the owner's iCloud Drive `Downloads` folder on the Mac. The build step reads it from the path in `ORACLE_ZIP` (default: that file); it is copied only into a temporary build directory, never into the repository. The owner confirms the license shown at download permits this use.
- **P2. Self-hosted runner.** Install a GitHub Actions runner for this repository on the OCI host:
  - labels `self-hosted`, `linux`, `arm64`, `saltation-oracle`;
  - it runs as a non-root user that can use Docker;
  - in repository settings, require approval for workflow runs from all outside contributors, so code from forks never runs on the host.
  The agent provides the exact commands in `docs/dev-host.md` (step 9); the owner runs them, because registration needs an owner token.
- **P3. Workflow handoff.** Step 9 changes `.github/workflows/repository.yml`. The GitHub App cannot push workflow files, so the owner pushes once, as `docs/host-setup.md` "Workflow changes" describes.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Content |
|---|---|
| `intent.md`, `spec.md` | replaced whole by the accepted packet copies; baseline marker removed (step 11) |
| `contracts/openapi/public.yaml` | login, logout, user info, account list, balance, transaction history, transfer, transfer result |
| `contracts/openapi/internal.yaml` | seed customers, seed accounts, clock read |
| `contracts/schemas/log-envelope.schema.json` | the F-37 envelope |
| `contracts/codes/*.json` | response codes, `inout_type`, `tran_type`, account type, the synthetic `bank_code_std` |
| `bank/Cargo.toml`, `bank/Cargo.lock`, `bank/Dockerfile`, `bank/rust-toolchain.toml` | Rust 2024 edition, pinned toolchain; multi-stage image with Oracle Instant Client for arm64 and amd64 |
| `bank/migrations/0001_*.sql` … | institutions, products and terms, codes, customers, credentials, devices, auth sessions, login attempts, accounts, fintech accounts, access tokens, transaction requests, transfers, account transactions, daily totals, sim clock, outbox |
| `bank/src/main.rs`, `bank/src/app.rs` | config, three listeners (public 8080, generator 8081, metrics 9100), router composition |
| `bank/src/shared/{db,ledger,clock,outbox,auth,context,api,telemetry}/` | as in the spec layout; `shared/ledger/sql/` holds lock, insert-row and balance statements |
| `bank/src/features/{auth,accounts,transfers,internal,outbox_relay}/` | each with `mod.rs`, handlers, rules, repository and `sql/` |
| `bank/tests/` | integration tests against Oracle and MongoDB |
| `web/package.json`, `web/pnpm-workspace.yaml`, `web/pnpm-lock.yaml`, `web/eslint.config.js`, `web/tsconfig.base.json` | pnpm workspace |
| `web/packages/api-client/` | types generated from `contracts/openapi/public.yaml` |
| `web/apps/mobile/` | Vite + React + TypeScript; features `auth`, `accounts`, `transfers`; Playwright smoke test |
| `generator/pyproject.toml`, `generator/uv.lock`, `generator/saltation_gen/{client,drivers,personas,config,truth}/`, `generator/tests/` | seed driver and a normal-transfer driver; `truth` writes `normal` labels only |
| `infra/oracle/` | `build.sh` (pinned `oracle/docker-images` commit), init SQL (tablespaces including `OUTBOX_TS` max 4 GB, undo max 4 GB, NOARCHIVELOG, redo 3 × 1 GB, users), `smoke.sh`, README |
| `infra/compose/compose.yml`, `infra/compose/.env.example` | services `oracle`, `mongo`, `bank`, `web`, `alloy`, `loki`, `prometheus`, `grafana` (`cloudflared` arrives in stage 6); networks `data`, `edge`, `internal`, `obs`; `local` profile with localhost ports; per-service CPU and memory limits from N-7; health checks |
| `infra/nginx/` | static serving of the mobile app |
| `infra/observability/{alloy,loki,prometheus,grafana}/` | configs, retention from N-9, datasources, one dashboard (request rate, p95, errors, outbox lag) |
| `infra/mongo/init/` | `bank_logs` and `truth` databases and users |
| `loadtest/` | not in this change |
| `scripts/dev/` | `ci-rust.sh`, `ci-web.sh`, `ci-py.sh`, `it-oracle.sh`: container wrappers that the checks call |
| `checks.json` | new checks (step 9) |
| `.github/workflows/repository.yml` | jobs `rust`, `web`, `python` (GitHub-hosted) and `oracle-integration` (self-hosted); added to `summary.needs` |
| `docs/dev-host.md` | host prerequisites, runner setup commands, how to run the stack and the tests |
| `.gitignore` | `target/`, `node_modules/`, `dist/`, `.venv/`, `infra/compose/.env`, Oracle zip patterns |
| `changes/product-init/**` | this packet |

## Order of work

Each step names its proof command, run from the repository root on the Mac. A step is done when its command passes. Commit after each step.

1. **Oracle image.** Start Colima with the sizes above. Write `infra/oracle/build.sh`. It clones `oracle/docker-images` at a pinned commit into a temporary directory, copies the zip from `ORACLE_ZIP`, and builds `saltation/oracle-ee:19.19` for linux/arm64. Write the init SQL and `smoke.sh`.
   - Files: `infra/oracle/**`.
   - Proof: `infra/oracle/build.sh && infra/oracle/smoke.sh`. The smoke test prints the 19c banner, NOARCHIVELOG mode, `OUTBOX_TS` max size 4 GB and the undo max size.
2. **Compose data services.** Add `oracle` and `mongo` with health checks, limits, networks and the Mongo init that creates two users.
   - Files: `infra/compose/**`, `infra/mongo/**`.
   - Proof: `docker compose -f infra/compose/compose.yml --profile local up -d --wait oracle mongo && scripts/dev/it-oracle.sh mongo-users`. The test logs in with each Mongo user and shows access denied on the other user's database.
3. **Rust skeleton and Oracle connection.** Add the Cargo project, `Dockerfile`, the shared `db` module (pool, transaction helper, statement runner with timing) and the migration runner with migrations `0001`–`0004`.
   - Files: `bank/**`, `scripts/dev/ci-rust.sh`.
   - Proof: `scripts/dev/it-oracle.sh migrations`. This runs `cargo test --test migrations` in a container on the `data` network: migrations apply to an empty schema, are recorded, and re-running applies nothing.
4. **Clock, context, listeners, envelope.** Add `shared/clock` (persisted `SIM_CLOCK`, live mode only in this stage), `shared/context`, `shared/api`, `shared/telemetry`, the three listeners and `/healthz`.
   - Proof: `scripts/dev/it-oracle.sh listeners`. Tests show:
     - simulated-context headers take effect on 8081 and are ignored on 8080;
     - `/internal` returns 404 on 8080;
     - `/metrics` exists only on 9100;
     - every response carries `request_id`.
5. **Seeding and login.**
   - Add `features/internal` (seed customers with credentials, seed demand accounts with `fintech_use_num`, read the clock).
   - Add `features/auth`: login with Argon2id, token, session, device, login attempts, five-failure lockout, logout.
   - Add `shared/outbox` writing events in the same transaction.
   - Proof: `scripts/dev/it-oracle.sh auth`. Tests cover success, wrong password, unknown phone, lockout at five failures, and outbox events for each attempt.
6. **Balance and history.** Add `features/accounts`: user info, account list, balance and transaction history, with KFTC field names, types, lengths, pagination and response codes.
   - Proof: `scripts/dev/it-oracle.sh accounts`. Responses validate against `contracts/openapi/public.yaml`.
7. **Transfer.**
   - Add `shared/ledger`: account locks in ascending ID order, rows, balances, daily totals.
   - Add `features/transfers`: in-bank transfer and transfer result, with idempotency on `bank_tran_id` and the request hash, rejections recorded, and the daily limit.
   - Proof: `scripts/dev/it-oracle.sh transfers`. Tests cover:
     - success, insufficient balance, same account, limit;
     - same key with the same payload returns the stored result; with a different payload it is rejected;
     - 200 concurrent duplicate requests produce one money movement;
     - concurrency: 1,000 random two-way transfers among 20 accounts on 32 tasks, with no deadlock error, the total balance unchanged, every `TRANSFER` offsetting, and every account's rows summing to its balance.
8. **Outbox relay and logs.**
   - Add `features/outbox_relay`: batches of 500, bulk upsert with journaled acknowledgement, delete on delivery, metrics for undelivered count and lag.
   - Add the `app.log` writer: one JSON line per request, daily rotation, 7 files.
   - Add the observability services and configs.
   - Proof: `scripts/dev/it-oracle.sh outbox`. Tests show:
     - events delivered exactly once;
     - the outbox is empty after delivery;
     - after a relay restart mid-batch, there are no duplicates in `bank_logs`;
     - with MongoDB stopped for 2 minutes during a transfer load, events accumulate under the cap and fully deliver after restart.
   - Then `docker compose --profile local up -d --wait` and `scripts/dev/it-oracle.sh observability`. This queries Loki for a `request_id` and Prometheus for the bank target and transfer histogram.
9. **Contracts, web, generator, checks and CI.**
   - Contracts and the generated `api-client`.
   - The mobile app (login, accounts, balance, history, transfer) and nginx.
   - The generator: `seed --customers 1000` and `run-normal --minutes 10 --rate 5`, labelling every request `normal` in `truth`.
   - The boundary rules:
     - Rust: feature modules expose only their router and registrations; other items are private to the feature, so the compiler rejects cross-feature use.
     - Web: ESLint `import/no-restricted-paths`.
     - Generator: `import-linter` contracts.
   - Register the checks (see Tests and proof). Write `docs/dev-host.md`, including the P2 runner commands.
   - Proof: `python3 scripts/repo.py verify --change product-init --full`. Every check passes, except `oracle-integration` until P2 is done; it runs locally through `scripts/dev/it-oracle.sh all`.
10. **Skeleton end to end.**
    - `docker compose --profile local up -d --wait`.
    - The generator seeds 1,000 customers and runs normal transfers for 10 minutes.
    - A person drives the mobile app through login, balance, history and transfer; the Playwright smoke test does the same headlessly.
    - Proof: `scripts/dev/it-oracle.sh e2e`. It checks:
      - generator requests have `normal` labels for every subject;
      - the outbox is drained;
      - every `TRANSACTION_REQUEST` has its `request.completed` event in `bank_logs`;
      - every `TRANSFER` offsets;
      - balances equal the sum of rows;
      - Grafana's dashboard query returns data;
      - the Playwright smoke test passes.
11. **Baseline.** Replace root `intent.md` and `spec.md` with the packet copies (no marker).
    - Proof: `python3 scripts/repo.py status --change product-init`.
12. **Workflow handoff and pull request.**
    - Commit the workflow change as its own commit, with more commits on top, and hand its SHA to the owner (P3).
    - After the owner's push, push the rest, run `repo.py verify --change product-init --full`, and open the pull request with `Change-ID: product-init`.
    - Proof: the CI `summary` check passes, including `oracle-integration` on the self-hosted runner.

## Tests and proof

New checks in `checks.json`:

| id | group | runs | paths |
|---|---|---|---|
| `rust-fmt` | `rust` | `scripts/dev/ci-rust.sh fmt` (`cargo fmt --check`) | `bank/**` |
| `rust-clippy` | `rust` | `scripts/dev/ci-rust.sh clippy` (`-D warnings`) | `bank/**` |
| `rust-unit` | `rust` | `scripts/dev/ci-rust.sh test` (unit tests, no database) | `bank/**`, `contracts/**` |
| `oracle-integration` | `oracle` | `scripts/dev/it-oracle.sh all` | `bank/**`, `infra/**`, `contracts/**`, `generator/**`, `web/**` |
| `web-lint` | `web` | `scripts/dev/ci-web.sh lint` (ESLint including boundaries) | `web/**`, `contracts/**` |
| `web-typecheck` | `web` | `scripts/dev/ci-web.sh typecheck` | `web/**`, `contracts/**` |
| `web-test` | `web` | `scripts/dev/ci-web.sh test` (Vitest) | `web/**` |
| `web-build` | `web` | `scripts/dev/ci-web.sh build` | `web/**` |
| `py-lint` | `python` | `scripts/dev/ci-py.sh lint` (Ruff check and format check, import-linter) | `generator/**` |
| `py-test` | `python` | `scripts/dev/ci-py.sh test` (pytest) | `generator/**` |
| `compose-config` | `core` | `docker compose -f infra/compose/compose.yml --profile local config -q` | `infra/**` |

- **Boundary proof:** each boundary check is shown failing once on a deliberate cross-feature import, as the `repository-quality` skill requires. The failing output is recorded in the PR description.
- **Evidence:** `repo.py verify --full` evidence under `.evidence/`, the step 7, 8 and 10 test outputs, the Grafana screenshot, and the Playwright report.

## Risks and mitigations

- **Oracle 19c ARM64 build fails or is slow.** It is step 1, so it fails before anything depends on it. Pin the `docker-images` commit. If the build is impossible on this host, stop and revise the plan with the owner.
- **The `oracle` crate on ARM64** needs Instant Client for Linux ARM64. Step 3 proves the connection before any feature code.
- **Self-hosted runner security.** Docker access is root-equivalent on a shared host. Mitigations: approval required for outside contributors; the runner is used only by the `oracle-integration` job; the job cleans up its containers and volumes; no secrets beyond the test database passwords. The owner may stop the runner service at any time; CI then reports `oracle-integration` as not completed rather than passing.
- **Mac resources.** The Colima VM gets 6 of 10 CPUs and 10 of 16 GB; Compose limits from N-7 apply inside it. The full N-7 budget (14 GB) does not fit on the Mac, so Loki and Prometheus run with reduced limits in the `local` profile; the full budget is exercised on the OCI host in stage 6.
- **Resource contention on the OCI runner.** The `oracle-integration` job uses a separate Compose project with its own volumes and the N-7 limits, and removes them afterwards.
- **Container-wrapped toolchains are slower.** Cargo, pnpm and uv caches are mounted as named volumes.
- **Scope creep from later stages.** Anything outside the Delivery stage-1 row waits for its own change.

## Rollback or recovery

- Before merge: close the pull request; the branch can be deleted.
- After merge: revert the squash commit in a new change.
- On the Mac and the OCI host:
  - `docker compose -f infra/compose/compose.yml --profile local down -v` removes the stack and its data;
  - `docker image rm saltation/oracle-ee:19.19` removes the image;
  - `colima stop` frees the Mac's resources;
  - the owner removes the runner from the OCI host with `./config.sh remove`.
- No co-tenant service or file is modified.

## Open questions

- None.
