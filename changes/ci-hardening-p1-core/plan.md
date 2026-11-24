# Plan: Harden CI Failure Logs, Timeout Cleanup, PR Readiness Gate, Playwright Isolation, and Midnight Guard (Packet 1)

## Goal

Fix the highest-severity agent-pushable CI reliability, isolation, and operability defects identified by the Carbon + Astra audit:
1. Eliminate CI failure blindness in `scripts/repo.py` by emitting per-check failure log tails on `failed`/`blocked` checks.
2. Send `SIGTERM` (with 15s grace) before `SIGKILL` on check timeout in `scripts/repo.py` so container cleanup traps can execute.
3. Enforce `readiness == "ready"` in PR status mode (`scripts/repo.py`) so unapproved packet-only PRs cannot pass `status --ci`.
4. Reclaim OCI RAM and unblock `check_compose_budget.py runtime` by stopping `saltation-it` `oracle` and `mongo` in `cleanup_it()` (preserving warm volumes), prune `saltation-it_applogs` in `reset_schema()`, label all raw `docker run` containers, isolate Playwright against a read-only `/web-src:ro` mount (zero root writes to the host checkout), drop redundant `--build` calls, and close inherited fd `9` in background subshells (`scripts/dev/it-oracle.sh`).
5. Eliminate the nightly `23:56:30–23:59:59 KST` `generator-lifecycle` blackout in `generator/tests/stage6_lifecycle_harness.py`.

No `.github/workflows/**` files are modified in this packet.

## Scope

- `scripts/repo.py`
- `scripts/tests/test_status.py`
- `scripts/tests/test_verify.py`
- `scripts/dev/it-oracle.sh`
- `generator/tests/stage6_lifecycle_harness.py`

## Order of work

1. **Harden `scripts/repo.py` failure visibility, timeout grace, and PR readiness gate (`scripts/repo.py`, `scripts/tests/test_verify.py`, `scripts/tests/test_status.py`)**
   - On `subprocess.TimeoutExpired` in `run_check()`, send `os.killpg(proc.pid, signal.SIGTERM)`, wait up to 15s (`proc.wait(timeout=15)`), and escalate to `SIGKILL` only on a second timeout.
   - In non-JSON `cmd_verify` output, print per-check duration and, when a check is `failed` or `blocked` with an existing `log` file, print a bounded tail (last 150 lines, capped at 32 KiB) directly to stdout so GitHub Actions logs always show the error on both `ubuntu-latest` and OCI.
   - In `pull_request_failures()`, fail if `st.readiness != "ready"` so unapproved packet-only PRs cannot pass PR status.
   - Add unit tests in `scripts/tests/test_verify.py` (failure log tail output + `SIGTERM`-before-`SIGKILL` grace) and `scripts/tests/test_status.py` (unapproved packet-only PR fails PR status).
   - Proof: `python3 scripts/tests/run.py`.

2. **Harden `scripts/dev/it-oracle.sh` OCI isolation, `applogs` hygiene, Playwright `:ro` mount, and fd `9` safety (`scripts/dev/it-oracle.sh`)**
   - In `reset_schema()`: after Oracle/Mongo reset, clear `/var/log/saltation/app*.log` inside `${project}_applogs` (via a short container using the `bank` image or `alloy` volume mount while `bank` is stopped) so a partial JSON line cannot poison CI for 7 days.
   - In `cleanup_it()`: after best-effort `reset_schema`, stop `oracle` and `mongo` (`"${compose[@]}" stop`) instead of leaving them running — reclaiming ~6–8 GB RAM on OCI and unblocking `check_compose_budget.py runtime` while keeping `saltation-it_oradata` and `saltation-it_mongodata` volumes warm.
   - Add `--label "com.docker.compose.project=$project" --label "com.docker.compose.oneoff=True"` to all raw `docker run` invocations (`curlimages/curl` and `playwright`) so Clean-Start and Clean-Exit always reap them.
   - In `browser_flow()` and `browser_dates()`: mount `$root/web` read-only at `/web-src:ro`, copy to container-local `/tmp/web`, enable `set -e`, validate `phone`/`password`/`to_account` are non-empty, and run Playwright inside `/tmp/web` (zero root-owned files on the host checkout, removing `chown -R` completely).
   - Add `ensure_web_image` build stamp and remove redundant `--build` from `observability()` and `browser_dates()` `compose up` calls.
   - Append `9>&-` to background subshells in `outbox_outage()` and `driver_bench()` and track/kill background PIDs in `cleanup_it()`.
   - Proof: `bash -n scripts/dev/it-oracle.sh`.

3. **Eliminate KST midnight blackout in `generator/tests/stage6_lifecycle_harness.py`**
   - Before sampling `started = datetime.now(UTC)` in `run_fixture()`, compute seconds until KST midnight; if `< 240` seconds remain, log a brief notice and `await asyncio.sleep(remaining + 2)` so the cutover and full `LIVE_SECONDS` window always start cleanly on the new KST date.
   - Proof: `(cd generator && uv run ruff check && uv run ruff format --check && uv run pytest tests/test_stage6_lifecycle_pacing.py)`.

## Risks and rollback

- **Risk 1 (Stopping `saltation-it` `oracle`/`mongo` on exit):** Adds ~30–45s warm DB startup at the start of the next CI run, in exchange for reclaiming ~6–8 GB RAM on the shared 24 GB OCI host and allowing `check_compose_budget.py runtime` to pass. Volumes are NOT removed (`stop`, never `down -v`).
- **Risk 2 (PR `readiness == "ready"` gate):** Could break an existing `test_status.py` test if any test expects an unapproved packet-only PR to pass `status --ci`.
  - **Mitigation:** Run `python3 scripts/tests/run.py` immediately and verify all existing tests (updating only if a test explicitly modeled a blocked PR).
- **Risk 3 (Playwright copy of `/web-src` to `/tmp/web`):** Must exclude `node_modules` if present on host (use `tar` or `cp` excluding `node_modules` / `.git` so the copy takes < 1s).
- **Rollback:** Revert the single PR commit; no application code, DB schema, or workflow files are modified.
