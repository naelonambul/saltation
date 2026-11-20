# Explain the documentation-only Oracle CI result

## Summary

Help agents reading the headless testing guide distinguish a required PR summary from evidence of an Oracle runtime test. Use this documentation-only change to verify the hosted route merged in PR #25.

## Implementation and proof

1. In `docs/agent-testing.md`, add a short note beside the check selection table. Explain where to find the Oracle job's documentation-only applicability message in GitHub Actions and link to the existing policy in `docs/host-setup.md`. Keep the local verification commands and their pass criteria intact.
2. Run `python3 scripts/repo.py status --change docs-ci-guide`, `git diff --check`, the native Oracle routing and workflow tests, and `python3 scripts/repo.py verify --change docs-ci-guide --full` on the exact candidate. Clean only the owned resources of its isolated pre-push verification. Check that the effective PR diff contains exactly `docs/agent-testing.md`, `changes/docs-ci-guide/change.json` and `changes/docs-ci-guide/plan.md`.
3. Open the documentation-only PR under the agent identity with the single `Change-ID: docs-ci-guide`. Record the base, head and merge-candidate SHAs and the GitHub run attempt. Require a successful `oracle` job assigned to GitHub-hosted `ubuntu-latest` in job metadata or setup output, an applicability log bound to the same candidate, and the successful required `summary`. A green summary alone leaves hosted verification unconfirmed. Record the run and job links, and compare the observed job and workflow time with PR #24's documentation-only full Oracle run. Keep PR #25's post-merge main run as the separate full-OCI control.

## Rollback

Revert the note if it becomes inaccurate. This change modifies documentation only.
