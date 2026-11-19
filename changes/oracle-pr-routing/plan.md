# Route documentation-only PRs away from OCI

## Summary

PR #24 changed the agent documentation and its change packet. Its Oracle job took 20m22s. The OCI warm-cache experiment found little benefit in prefetching browser dependencies. Route a proven documentation-only PR's existing `oracle` job to a GitHub-hosted runner. Record the reason and the commit identities in that job. Keep `summary` as the required check and keep its all-success rule. Run the full Oracle group for every other PR and every `main` push. The weekly fresh Oracle workflow continues separately.

Astra and Argon recommended routing the existing job. Neon proposed allowing an explicitly authorized skip in `summary`. This plan follows the first recommendation and retains the current job-success invariant.

## Files and order of work

1. Create `scripts/oracle_pr_route.py` as a Git-only decision command. Accept the trusted event type, PR base SHA, PR head SHA, checked-out candidate SHA, and PR body file. Default to `full`. Accept `docs-only` only for a pull-request merge candidate whose parents match the event base and head. Verify the actual checkout, compute the head's merge base against the event base, and inspect both merge-base-to-head and event-base-to-candidate diffs. Use NUL-delimited raw paths and modes with renames disabled, so a move includes both endpoints. A Git error, missing ref, empty comparison, unexpected candidate, or unreadable PR identity results in `full`.

   Qualifying paths are exactly root `README.md`, `README.ko.md`, ordinary Markdown files under `docs/`, and `change.json` or `plan.md` under the one packet identified by the PR's `Change-ID`. Reuse the control-plane Change-ID parser. Require at least one README or docs file in each comparison. Permit only regular non-executable file content or add/delete operations on those paths. Type and mode changes, symlinks, submodules, unsupported paths, packet spec/intent changes, and any product or policy path require `full`. Emit one of the two fixed modes; put the reason, candidate identities and safely escaped paths in logs.

2. In `.github/workflows/repository.yml`, have the hosted `status` job emit the decision after its existing checks; preserve their failures. Give `oracle` a dependency on `status` and select runners from two fixed label lists. Invoke the existing `repo.py verify --full --group oracle` on the OCI route. On the hosted route, reproduce the documentation-only decision for the same checked-out candidate and fail on disagreement. Record `Oracle integration: not applicable (documentation-only)` with the exact base, head and candidate. Keep the existing workflow triggers, job names, registry checks, PR cancellation behavior, and strict `summary`. The hosted path must never record an Oracle runtime check as passed. A missing routing output chooses the full route when status succeeded. A failed or cancelled status prevents the required summary from passing.

3. Add Git-fixture tests in `scripts/tests/test_oracle_pr_route.py` and workflow assertions in `scripts/tests/test_workflow.py`. Cover guide plus active packet, docs-only rename, docs plus code, code renamed into docs, deleted code, unknown paths, workflow or registry edits, foreign packets, packet-only changes, file modes and symlinks, stale or missing SHAs, empty or failed comparisons, and `main`/non-PR events. Assert that the full route keeps the original command and all success is still required. Run the focused tests, then `python3 scripts/tests/run.py` and `python3 scripts/repo.py verify --change oracle-pr-routing --full`.

4. In `docs/host-setup.md`, document how to tell a hosted documentation decision from a completed Oracle run. Follow its owner push handoff for the workflow commit. This PR changes CI and must use the full Oracle route. Confirm the exact final PR head and the post-merge `main` run retain the full OCI group and strict summary. Verify the hosted decision on the first qualifying docs-only PR using the updated workflow. Check the same PR's required summary and absence of OCI allocation; report the live hosted route as pending until then.

## Rollback

Restore the original OCI runner declaration and full check step. Keep the required summary strict during rollback.
