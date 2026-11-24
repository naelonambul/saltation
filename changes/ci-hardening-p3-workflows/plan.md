# Plan: Harden CI Workflow Security Boundaries for Self-Hosted OCI Runner and Weekly-Fresh (Packet 3)

## Goal

Close the remaining workflow-level security boundary gaps identified by the Carbon + Astra audit:
1. Prevent unverified or unauthorized PRs from executing on the Docker-privileged self-hosted `saltation-oracle` OCI runner before `identity` and `control-plane-tests` pass, and verify `status` using the base commit's `scripts/repo.py` alongside the candidate's.
2. Restrict manual `workflow_dispatch` runs of `.github/workflows/weekly-fresh.yml` to `refs/heads/main` only (preventing accidental execution of a feature branch with `--fresh` against the shared CI volumes) and include `scripts/dev/it-oracle.sh browser-dates` in the weekly fresh proof.

**Handoff required (`docs/host-setup.md` "Workflow changes"):** Because this change modifies `.github/workflows/**`, the App cannot push the workflow commit directly. The workflow commit will be created on `change/ci-hardening-p3-workflows` beneath a follow-up test commit (`scripts/tests/test_workflow.py`); the owner pushes up to the workflow commit SHA, and the agent pushes the top commit and opens the pull request.

## Scope

- `.github/workflows/repository.yml`
- `.github/workflows/weekly-fresh.yml`
- `scripts/tests/test_workflow.py`

## Order of work

1. **Harden `.github/workflows/repository.yml` (`oracle` gating & base-SHA `status` check)**
   - In `status`, on `pull_request`, run `$route_dir/scripts/repo.py status $REPO_ARGS` (extracted from `$PR_BASE_SHA`) before running the candidate `scripts/repo.py status $REPO_ARGS` so a PR cannot bypass `status` by modifying `scripts/repo.py`.
   - Change `oracle.needs` from `[status]` to `[status, identity, control-plane-tests]` so the self-hosted `saltation-oracle` runner is only allocated after `identity`, `control-plane-tests`, and `status` all succeed.
   - Proof: `python3 -m unittest -v scripts.tests.test_workflow`.

2. **Harden `.github/workflows/weekly-fresh.yml` (`main`-only dispatch & fresh `browser-dates`)**
   - Add a `refs/heads/main` guard step before running `it-oracle.sh all --fresh` so manual `workflow_dispatch` on any non-`main` ref fails immediately.
   - Add `scripts/dev/it-oracle.sh browser-dates` after `all --fresh` so the weekly fresh run proves mobile browser dates on fresh volumes too.
   - Proof: `python3 -m unittest -v scripts.tests.test_workflow`.

3. **Update `scripts/tests/test_workflow.py` (separate commit on top of workflow commit)**
   - Update `test_oracle_routes_only_proven_documentation_prs_to_hosted` to require `needs: [status, identity, control-plane-tests]` and base-SHA `repo.py status` execution.
   - Add a test asserting `weekly-fresh.yml` enforces `refs/heads/main` and runs `browser-dates`.
   - Proof: `python3 scripts/tests/run.py` and `python3 scripts/repo.py status --change ci-hardening-p3-workflows`.

## Risks and rollback

- **Risk 1 (`oracle` waiting for `identity` and `control-plane-tests`):** Adds ~25–35s before `oracle` starts on PRs, which is negligible compared to preventing untrusted PR code from reaching the Docker-privileged OCI host.
- **Risk 2 (Base-SHA `repo.py status` compatibility):** `$PR_BASE_SHA` already has the current `repo.py` schema and PR readiness rules (merged in PR #48).
- **Rollback:** Revert the single PR commit (via owner workflow handoff if reverting workflow files).
