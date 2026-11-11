# Implementation Plan

## Summary

Second change of the "small hard core plus optional profiles" direction (first: `core-lifecycle`, PR #53). The five reviews agreed that CI runs the whole control-plane suite twice, and that the template maintains its own test runner and an in-process/subprocess equivalence harness. Codex Astra and GPT Pro 6 added a constraint: keep one test path whose pass or fail does not depend on `repo.py verify`, because `verify` must not be the only judge of its own tests.

This change:

- runs the control-plane tests **once** in CI, directly, outside `repo.py`;
- removes the `verify-core` job, which ran the same suite again through `repo.py verify`;
- replaces the 263-line parallel sharding runner with a short wrapper around native `unittest` that keeps the one guarantee worth keeping: a run that ends early, or runs zero tests, fails;
- removes the in-process call path from the test harness: tests run `repo.py` and `guard.py` as real subprocesses, as users do;
- replaces the line-grammar allowlist and mutation matrix in `test_workflow.py` with a few semantic assertions;
- lets `repo.py status` accept a check group as wired when CI runs its checks directly, not only through `repo.py verify --group`.

### Measured cost (2026-11-09, 10-core Mac, Python 3.13, 237 tests after `core-lifecycle`)

| Command | Wall time |
|---|---|
| `python3 scripts/tests/run.py` (current parallel runner) | 9 s |
| `python3 -m unittest discover -s scripts/tests -t .` (in-process calls, sequential) | 30 s |
| the same with `SDLC_TEST_SUBPROCESS=1` (real subprocesses, sequential) | 41 s |

Local runs get about 30 s slower, and CI on a 4-core runner gets somewhat slower too. In exchange the template drops about 1,300 lines of runner and harness code and their tests, and CI runs the suite once instead of twice.

## Files and components that change

- `scripts/tests/run.py`: rewrite, about 40 lines. It runs `sys.executable -m unittest discover -s scripts/tests -t .` in a child process from the repository root, streams its output, and exits 0 only if the child exited 0, its output's final summary is `Ran N test(s)` with N > 0, and the result line starts with `OK`. Otherwise it exits 1 with one line saying why. Extra arguments go through to `unittest` (for example `-v`, or `-k`).
- `scripts/tests/test_run.py`: rewrite, about 80 lines. Each case runs `run.py` against a temporary test directory, through a `--start` option for the discovery directory: passing tests pass; a failing test fails; a test that calls `os._exit(0)` fails; a directory with zero tests fails; an import error fails; arguments pass through.
- `scripts/tests/support.py`: delete `mode`, `load`, `_stream`, `_text`, `invoke`, `tree_state`, `restore`, `normalize`, `_normal_state`, `compare_runs`, `RepoCase.run_both`, and the `SDLC_TEST_SUBPROCESS`/`SDLC_TEST_EQUIVALENCE` switches. `RepoCase.run_script` always calls `run_subprocess`. Keep `run_subprocess`, `run_repo`, `run_repo_subprocess` (an alias of `run_repo`), fixtures and helpers.
- `scripts/tests/test_support.py`: delete `CompareRuns`, `TreeState` and `InProcessIsolation`. Keep `SubprocessContract` and `NoAutoMaintenance`.
- `scripts/tests/test_workflow.py`: delete `JOB_LINES`, `TOP_LEVEL_KEYS`, `TOP_LEVEL_ENV`, `control_plane_job_findings` and the mutation test. Keep the trigger, pinning, full-history, concurrency, job-condition, summary and identity tests. Add one semantic test: the `control-plane-tests` job has a `run:` step equal to the `control-plane` check's argv joined by spaces, and the job contains no `repo.py`, `continue-on-error`, step-level `if:`, `|| true` or `shell:`. Replace `test_every_check_group_is_run` with a test that calls `repo.py status` wiring logic on the real workflow and `checks.json` and expects no `checks-unwired` finding.
- `scripts/repo.py`, `config_findings`: a group is wired when the uncommented workflow runs `repo.py verify --group <group>`, or when every check in the group appears as a `run:` line equal to its argv joined by spaces. A workflow `verify --group` for an undeclared group stays an error. Update the `checks-unwired` message to name both forms.
- `scripts/tests/test_status.py`, `SmallGates`/`CheckWiring`: add cases. A group run directly is wired. A group with one of two checks run directly is unwired. A commented direct run is not wired. Existing `verify --group` cases are unchanged.
- `.github/workflows/repository.yml`:
  - delete the `verify-core` job and the `VERIFY_ARGS` env entry;
  - `control-plane-tests` keeps one step, `python3 scripts/tests/run.py`, and drops `python3 -m unittest scripts.tests.test_run`, since `run.py` runs `test_run` as part of the suite;
  - `summary.needs` becomes `[control-plane-tests, status, identity]`;
  - fix the stale comment on `status`'s `fetch-depth: 0` (the PR base needs history; closure anchors no longer exist).
- `scripts/README.md`: describe the new `run.py`, drop the shards, `--jobs`, `SDLC_TEST_JOBS` and equivalence text.

`checks.json` does not change: the `control-plane` check keeps `["python3", "scripts/tests/run.py"]`, so local `repo.py verify` still runs the suite and records evidence.

## Order of work

Each step ends with the command that proves it.

1. `scripts/tests/run.py` and `test_run.py`. Proof: `python3 -m unittest scripts.tests.test_run` passes and `python3 scripts/tests/run.py` reports `OK` with the current test count.
2. `support.py` subprocess-only, and `test_support.py`. Proof: `python3 scripts/tests/run.py` passes; `git grep -n "SDLC_TEST_\|compare_runs\|run_both\|invoke(" scripts` returns nothing.
3. `repo.py` `config_findings`, plus the new `test_status.py` cases. Proof: `python3 -m unittest scripts.tests.test_status`.
4. `test_workflow.py` rewrite. Proof: `python3 -m unittest scripts.tests.test_workflow`, with the workflow still unchanged. The new direct-run test passes against today's job, which already runs `python3 scripts/tests/run.py`.
5. `scripts/README.md`. Proof: `git grep -n -E "SDLC_TEST|--jobs|shard" -- scripts/README.md` returns nothing.
6. Commit steps 1-5. Then make the workflow edit as **its own commit**, with no other change, and add one commit on top that records the verify result in this packet's plan (Verification section). Proof: `python3 scripts/repo.py status --change ci-native-tests` passes (no `checks-unwired`), and `python3 scripts/repo.py verify --change ci-native-tests --full` reports `control-plane` passed.
7. Workflow handoff (`docs/host-setup.md`, "Workflow changes"). The agent pushes the commits before the workflow commit and gives the owner the workflow commit's SHA. The owner pushes exactly that SHA. The agent confirms the remote head with `git ls-remote`, then pushes the commit on top and opens the pull request.

## Tests and proof

- `test_run.py` covers each condition in which the wrapper must fail: non-zero exit, `os._exit(0)` mid-run, zero tests, import error. It also covers the pass case.
- `test_status.py` covers the new wiring rule both ways.
- `test_workflow.py` asserts on the real workflow that the tests run directly and cannot be muted by the listed constructs, that `summary` needs every job, and that no group is unwired.
- Whole change: `python3 scripts/repo.py verify --change ci-native-tests --full` complete and passed, and CI `summary` green on the pull request, with the new job set.

## Risks and mitigations

- **Loss of guarantee: the parallel runner's per-class process isolation and exact per-test accounting.** The new wrapper checks only that `unittest` printed a final `OK` summary with N > 0 tests. A test that silently skips itself still counts. Mitigation: skips are printed in the summary (`OK (skipped=N)`), and review sees them. Accepted.
- **Loss of guarantee: the in-process path's equivalence check.** That path existed only for speed. With it gone, there is nothing to compare.
- **Slower local and CI runs** (measured above). Mitigation: `run.py` passes `-k` through, for focused local runs.
- **Weaker structural lock on the workflow file.** The line-grammar allowlist rejected constructs a plain-text check cannot read, such as `defaults:` and block scalars. The new test checks named constructs only. Mitigation: the owner pushes every workflow change (the App cannot), and `summary` still requires every job. A workflow edit is reviewed as a workflow edit. Accepted, as the reviews recommended.
- **Riskiest step: the workflow commit**, because it changes the merge gate itself. Mitigation: it is one commit with no other content; the pull request's own CI runs the new workflow before merge; `summary` must be green.

## Rollback or recovery

A new change that restores the four test files, `support.py` and the workflow from this change's squash commit parent. A plain `git revert` of the squash commit is acceptable, and the workflow part needs the owner push again.

## Open questions

None.
