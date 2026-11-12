# Implementation Plan

## Summary

Since `oracle-ci-speed`, CI and local runs reuse the Oracle test database, and only a setup-key change recreates it. A slow drift (a migration that only works on a reused schema, an image or Docker change that breaks database creation) would stay hidden. This change runs the whole Oracle integration suite from empty volumes once a week on the self-hosted CI runner, against `main`.

- New workflow `.github/workflows/weekly-fresh.yml`: on a weekly schedule (Sunday 04:00 KST) and on manual dispatch, it checks out `main` on the `saltation-oracle` runner and runs `scripts/dev/it-oracle.sh all --fresh`.
- The result is the workflow run in GitHub Actions; GitHub emails a failed scheduled run to the owner, who pushes the workflow file.

Decisions:

- **One place, the CI runner.** Development happens on the Mac mini and on the OCI host, so a weekly run on the runner covers both, needs no host-specific installer, and records its results where the other CI results are.
- **No change to `it-oracle.sh` or `repository.yml`.** The new workflow calls the script with `--fresh`; the script's lock keeps it from overlapping a pull-request `oracle` job, and the fresh database it leaves behind is reused by the next one.
- **Not a required check.** The weekly workflow does not feed the `summary` job, so it never blocks a pull request.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `.github/workflows/weekly-fresh.yml` | new: weekly and manual fresh Oracle run on the self-hosted runner |
| `docs/dev-host.md` | the CI section describes the weekly run, how to start it by hand and where to read results |

## Order of work

1. **Workflow.** `.github/workflows/weekly-fresh.yml`:
   - `on: schedule: - cron: "0 19 * * 6"` (Saturday 19:00 UTC = Sunday 04:00 KST) and `workflow_dispatch`.
   - `permissions: contents: read`; `concurrency: group: weekly-fresh, cancel-in-progress: false`.
   - One job `oracle-fresh` on `[self-hosted, linux, ARM64, saltation-oracle]`, `timeout-minutes: 120`: `actions/checkout` and `actions/setup-python` pinned to the same SHAs as `repository.yml` (`persist-credentials: false`), then `scripts/dev/it-oracle.sh all --fresh`.
   - Proof: actionlint (the `rhysd/actionlint` Docker image, pinned by digest) reports nothing for `.github/workflows/weekly-fresh.yml`, and `scripts/dev/it-oracle.sh all --fresh`, the command the job runs, passes locally.
2. **Docs.** `docs/dev-host.md`, CI section: what the weekly run proves, its schedule, how to start it (`gh workflow run weekly-fresh.yml`, owner), that a failure arrives by GitHub email, and that GitHub disables scheduled workflows after 60 days without repository activity.
3. **Workflow handoff** (`docs/host-setup.md`, "Workflow changes"): commit the workflow as its own commit, commit the docs on top, run `python3 scripts/repo.py verify --change weekly-fresh-oracle --full` untouched, give the owner the workflow commit SHA; the owner pushes up to it, the agent pushes the rest and opens the pull request.
4. **After merge:** the owner starts it once with `gh workflow run weekly-fresh.yml`; record its duration and result on the pull request.

Implementation is delegated: Fable leads and briefs coding workers run as `codex exec --yolo -m gpt-6.1-sol -c model_reasoning_effort=low`; workers stay inside `write_scope` and never commit. Mac Claude reruns every proof and the full verify before committing.

## Tests and proof

The proofs above; the full verify; the CI `summary` check on the pull request; the first dispatched run after merge passes and its log shows `it-oracle: creating fresh test volumes`.

## Risks and mitigations

- **Overlap with a pull-request `oracle` job:** the `it-oracle.sh` lock serialises them; one waits (about 12 minutes at most).
- **The weekly run wipes the runner's test database:** intended; it is test data only, and the next job reuses the new one.
- **Silent disablement after 60 days of inactivity:** documented; the owner re-enables it in the Actions tab.
- **Workflow security:** schedule and dispatch run only on `main` code, with read-only permissions and no secrets; fork pull requests cannot trigger it.

## Rollback or recovery

Disable the workflow in the Actions tab, or revert the squash commit.

## Open questions

- None.
