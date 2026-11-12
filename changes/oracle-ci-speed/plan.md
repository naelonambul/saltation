# Implementation Plan

## Summary

The `oracle` CI job takes 10 to 18 minutes, mostly because every run creates a new Oracle database: `actions/checkout` deletes the untracked `infra/compose/.env.it`, new passwords are generated, and `it-oracle.sh` recreates the test volumes. Fable, Astra and Argon independently recommended the same design; this change implements it, without touching the workflow or `compose.yml`.

- Keep the test env file and volumes on the runner between runs, so the database is created once and reused.
- Recreate them when anything that only runs at volume creation changes (a setup key), on `--fresh`, or when the env file is new.
- Serialise runs on a host with a lock, and clean up state a cancelled run may leave behind.
- Build the dev image once per script run and run the post-migration Rust targets in one `cargo test` invocation.
- Stop Docker build contexts from carrying test files, `.git` and evidence, so unchanged images stay cached.

Out of scope, deferred to a later change on the Mac mini: a scheduled weekly run from an empty database. Until then a setup-key change still forces a fresh database, so the setup scripts are proven whenever they change.

Target: about 2 to 4 minutes for a typical run (unverified until measured on the runner).

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `scripts/dev/it-oracle.sh` | state directory, env file and lock outside the checkout; setup key; cleanup at start; dev image built once; batched cargo targets; observability containers always recreated |
| `bank/.dockerignore` | exclude `target`, `tests` and editor files from the release image context |
| `web/Dockerfile.dockerignore` | also exclude `.evidence`, `changes`, `scripts`, `docs` |
| `docs/dev-host.md` | state location, the setup key, recovery with `--fresh`, and timing |

## Order of work

1. **State outside the checkout and a lock.** `state_dir="${SALTATION_IT_DIR:-$HOME/.cache/saltation-it}"` (mode 0700) holds `env` and `lock`; `exec 9>"$state_dir/lock"; flock 9` when `flock` exists. The previous `infra/compose/.env.it` is no longer used.
   - Proof: two concurrent `scripts/dev/it-oracle.sh mongo-users` runs complete one after the other (the second waits for the lock).
2. **Setup key.** `SETUP_KEY` = SHA-256 over `infra/oracle/setup/*`, `infra/mongo/init/*`, `infra/oracle/build.sh` and the Oracle image ID, stored in the env file. A missing env file, a different key or `--fresh` runs `down -v` first.
   - Proof: a second run with nothing changed reuses the database (no "% complete" DBCA lines); touching a comment in `infra/oracle/setup/01_saltation.sql` makes the next run recreate it.
3. **Cleanup at start.** Remove a leftover `saltation-it-cargo` container; stop `bank web alloy loki prometheus grafana`; start `oracle mongo` (a cancelled `outbox-outage` may have stopped MongoDB); `reset_schema` and clear `bank_logs.events` once before the database targets.
   - Proof: `scripts/dev/it-oracle.sh all` passes right after a run cancelled during `outbox-outage`.
4. **Build once, run batched.** Build `saltation/bank-dev:local` once per script run; run `migrations` on the reset schema, then `listeners`, `auth`, `accounts`, `transfers` and `outbox` in one `cargo test --test ... -- --test-threads=1` in a container named `saltation-it-cargo`. Single targets still run alone.
   - Proof: `scripts/dev/it-oracle.sh all` passes and its log shows one dev-image build and two cargo invocations.
5. **Observability recreates its containers** (`up -d --build --force-recreate --wait`), so no container keeps a stale bind mount.
   - Proof: `scripts/dev/it-oracle.sh observability` passes after a branch switch.
6. **Build contexts.** Add `bank/.dockerignore`; extend `web/Dockerfile.dockerignore`.
   - Proof: editing only a file under `bank/tests/` leaves `docker compose build bank` fully cached.
7. **Docs.** Update `docs/dev-host.md`.
8. Run `python3 scripts/repo.py verify --change oracle-ci-speed --full` untouched; open the pull request; record the CI `oracle` job time for a cold run and for the following warm run.

## Tests and proof

The proofs above; the CI `summary` check; the warm `oracle` job time compared with the 10 to 18 minutes measured before.

## Risks and mitigations

- **State drift in a reused database:** the schema and MongoDB events are reset each run; the setup key recreates everything that only runs at volume creation; `--fresh` remains for manual recovery. No automatic retry with `--fresh` on failure, so state bugs stay visible.
- **Disk growth:** undo and the outbox are capped at 4 GB; Loki and Prometheus have retention; `DROP USER CASCADE` frees schema space for reuse.
- **Stored test passwords on the runner host:** mode 0600 in a 0700 directory, on a host where runner code is already trusted.
- **A cancelled run's leftovers:** handled by the start-of-run cleanup and the lock.

## Rollback or recovery

Revert the squash commit. On the runner, `scripts/dev/it-oracle.sh all --fresh` or deleting `$HOME/.cache/saltation-it` returns to a fresh state.

## Open questions

- None.
