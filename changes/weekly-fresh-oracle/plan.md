# Implementation Plan

## Summary

Since `oracle-ci-speed`, CI and local runs reuse the Oracle test database, and only a setup-key change recreates it. A slow drift (a migration that only works on a reused schema, an image or Docker change that breaks database creation) would stay hidden. This change runs the whole Oracle integration suite from empty volumes once a week on the owner's Mac mini, against the latest `main`, and reports the result on that Mac.

- `scripts/dev/weekly-fresh.sh`: in a dedicated clone, checks out `origin/main`, runs `scripts/dev/it-oracle.sh all --fresh`, keeps a log, records the result and shows a macOS notification.
- `scripts/dev/install-weekly-fresh.sh`: creates the clone and installs (or with `--uninstall` removes) a user launchd agent that runs it every Sunday at 04:00 local time.

Decisions:

- **Local result only.** No GitHub issue, comment or status is created; the result stays on the Mac (log, result file, notification). Publishing results can be a later change.
- **Separate clone.** The run never touches the owner's working tree. It fetches the public repository anonymously, so it needs no credentials.
- **Shared state.** It uses the default `it-oracle.sh` state directory, so its `--fresh` run leaves a fresh database that later local runs reuse, and the existing lock keeps it from overlapping a local run.
- **Self-updating.** The launchd agent updates the clone before it runs the script, so the script that runs is always the one on `main`.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `scripts/dev/weekly-fresh.sh` | new: preflight, fresh run, log rotation, result file, notification |
| `scripts/dev/install-weekly-fresh.sh` | new: clone, launchd agent install and uninstall |
| `docs/dev-host.md` | a "Weekly fresh run (Mac mini)" section |

Outside the repository, at install time only: the clone and logs in `${SALTATION_WEEKLY_DIR:-$HOME/.cache/saltation-weekly}` and `~/Library/LaunchAgents/com.saltation.weekly-fresh.plist`.

## Order of work

1. **`scripts/dev/weekly-fresh.sh`** (bash, macOS and Linux). Run from a checkout; uses its own checkout as the repository.
   - Preflight, each failure recorded as a failed result with its reason: `docker info` succeeds; the image `saltation/oracle-ee:19.19` exists.
   - Runs `scripts/dev/it-oracle.sh all --fresh`, output to `$SALTATION_WEEKLY_DIR/logs/<UTC timestamp>.log`; keeps the newest 12 logs.
   - Writes one line to `$SALTATION_WEEKLY_DIR/last-result`: `<UTC time> <commit> pass|fail <seconds>s <log path or reason>`, and appends it to `$SALTATION_WEEKLY_DIR/history`.
   - Shows a notification with `osascript` when it is available (title `Saltation weekly fresh run`, text pass or fail and the duration); exits with the run's status.
   - Proof: `scripts/dev/weekly-fresh.sh` from this branch passes, its log contains `it-oracle: creating fresh test volumes`, and `last-result` reads `pass`.
   - Proof (failure path): `PATH=/usr/bin:/bin scripts/dev/weekly-fresh.sh` (no `docker`) exits non-zero and `last-result` reads `fail` with the reason.
2. **`scripts/dev/install-weekly-fresh.sh [--ref <branch>] [--uninstall]`** (macOS only; refuses elsewhere).
   - Clones `https://github.com/naelonambul/saltation.git` into `$SALTATION_WEEKLY_DIR/repo` when missing.
   - Writes the plist: label `com.saltation.weekly-fresh`; `StartCalendarInterval` Weekday 0, Hour 4, Minute 0; a `PATH` that includes the directory of the current `docker`; standard output and error to `$SALTATION_WEEKLY_DIR/launchd.log`; the program is `/bin/bash -c` with: fetch `origin`, `git checkout --detach --force origin/<ref>` (default `main`), then run `scripts/dev/weekly-fresh.sh`.
   - Loads it with `launchctl bootstrap gui/<uid>` (after `bootout` of an earlier copy); `--uninstall` boots it out and deletes the plist, leaving the clone and logs.
   - Proof: `scripts/dev/install-weekly-fresh.sh --ref change/weekly-fresh-oracle`, then `launchctl print gui/$(id -u)/com.saltation.weekly-fresh` shows the calendar interval; `launchctl kickstart gui/$(id -u)/com.saltation.weekly-fresh` runs once and `last-result` reads `pass`; `--uninstall` removes it.
3. **Docs.** `docs/dev-host.md`: what the weekly run proves, install and uninstall, where the result and logs are, that the Mac must be awake with Docker Desktop running (launchd runs a missed time at the next wake).
4. Run `python3 scripts/repo.py verify --change weekly-fresh-oracle --full` untouched; open the pull request. After merge, the owner (or Mac Claude on request) reinstalls with the default `--ref main`.

Implementation is delegated: Fable leads and briefs coding workers run as `codex exec --yolo -m gpt-6.1-sol -c model_reasoning_effort=low`; workers stay inside `write_scope` and never commit. Mac Claude reruns every proof and the full verify before committing.

## Tests and proof

The proofs above; the full verify; the CI `summary` check (the change touches no registered check's code paths beyond `docs/dev-host.md`).

## Risks and mitigations

- **Mac asleep or Docker Desktop not running at 04:00:** launchd runs a missed calendar job at wake; a missing Docker is recorded as a failed result with the reason and a notification, not silently skipped.
- **Overlap with a local run:** the `it-oracle.sh` lock serialises them; the weekly run may wait.
- **A fresh run wipes the local test database:** intended; it is test data only, and the next local run reuses the new one.
- **`checkout --force` in the clone:** the clone is dedicated to this job and never edited by hand; the docs say so.
- **Disk:** 12 logs at most.

## Rollback or recovery

`scripts/dev/install-weekly-fresh.sh --uninstall`, then revert the squash commit. Deleting `$SALTATION_WEEKLY_DIR` removes the clone and logs.

## Open questions

- None.
