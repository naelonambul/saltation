# Plan — template-upgrade-f40b5e9

## Template upgrade scope

- **Upstream template repository:** `naelonambul/sdlc-project-template-agent` (local clone `../sdlc-project-template-agent`)
- **Base commit:** `51ca5887988429289bb33f03fa39faf5b67c2c28` (27 of 31 template-owned copy files matched)
- **Target commit:** `f40b5e90172501f9d61f145ed43eda20b1a3a66e` (includes `#55 scripts-comment-free`, `#56 codebase-rules-gate`, `#57 template-upgrade`, `#58 upgrade-adopter-tests`, `#60 closeout-byproducts`, and `#61 review-evidence`)

## Drifted & carried files (4 files)

Saltation carries 4 purposeful local improvements over base `51ca588`:

1. `scripts/repo.py` — graceful `SIGTERM` before `SIGKILL` on check timeout, duration formatting, and failure log tail printing (`format_log_tail`).
2. `scripts/tests/test_verify.py` — unit tests for failure log tail printing and `SIGTERM`-before-`SIGKILL` timeout handling.
3. `scripts/tests/test_status.py` — `test_unapproved_packet_only_pr_fails`.
4. `scripts/tests/test_workflow.py` — checks for `weekly-fresh.yml`, `persist-credentials: false`, `permissions: contents: read`, and `oracle-route` docs-only routing.

**Resolution:** Record all 4 paths in `checks.json` under `template.carried` and apply three-way merges (`git merge-file`) from `51ca588` to `f40b5e9` so Saltation's improvements are preserved while adopting upstream changes (including `repo.py upgrade`, `template-drift`, `gates`, and comment-free Python).

## Implementation steps

1. **Copy & symlink template-owned files (23 non-carried copy files)**
   - Apply all 23 non-carried `copy` entries from `f40b5e9` (including `.agents/skills/template-upgrade/`, `.agents/skills/sdlc-artifacts/references/structure.md`, `.claude/skills/template-upgrade` symlink, updated `changes/README.md`, `changes/_template/plan.md`, `scripts/hooks/guard.py` executable, `scripts/tests/test_comments.py`, `scripts/tests/test_upgrade.py`, and updated control-plane test files).
   - Preserve local-only `scripts/tests/test_oracle_pr_route.py`.

2. **Three-way merge carried & merge files (7 files)**
   - Merge the 4 `template.carried` files (`scripts/repo.py`, `scripts/tests/test_status.py`, `scripts/tests/test_verify.py`, `scripts/tests/test_workflow.py`) preserving Saltation's custom behavior and ensuring zero comments/docstrings.
   - Merge the 3 `merge` files (`AGENTS.md`, `REVIEW.md`, `.github/pull_request_template.md`) keeping Saltation's product-specific rules (including the `REVIEW.md` OTN Oracle benchmark clause) while adopting upstream `review.md` and performance-evidence sections.

3. **Migration 1 — `scripts-comment-free` (`4f99969`)**
   - Remove all comments and docstrings from Saltation-owned tracked Python files under `scripts/**/*.py` (`scripts/dev/**/*.py`, `scripts/ops/*.py`, `scripts/oracle_pr_route.py`, `scripts/tests/test_oracle_pr_route.py`), keeping only `#!/usr/bin/env python3` on executable (`100755`) files and encoding any load-bearing constraints in names/assertion messages/tests.

4. **Migration 2 — `codebase-rules-gate` (`500ab04`)**
   - Add `"gates"` to `checks.json` deciding all 7 roles:
     - `"build": "web-build"`
     - `"test": "rust-unit"`
     - `"lint": "rust-clippy"`
     - `"format": "rust-fmt"`
     - `"types": "web-typecheck"`
     - `"boundaries": "py-lint"`
     - `"comments": "comment-ratchet"`
   - Add a small comment-free ratchet checker `scripts/ops/check_new_comments.py` + `scripts/ops/test_check_new_comments.py` registered as `"comment-ratchet"` in the `core` group (Python + Git only), using Python's `tokenize`/`ast` to reject newly added comments/docstrings in diffed Python files unless listed in `comment-exceptions.json`.

5. **Migration 3 — `template-upgrade` (`8e3ce0d`)**
   - Compute the exact `"template"` pin (`commit`, `digest`, `carried`) via `python3 scripts/repo.py upgrade --template ../sdlc-project-template-agent --from 51ca5887988429289bb33f03fa39faf5b67c2c28 --to f40b5e90172501f9d61f145ed43eda20b1a3a66e` after staging the carried list, and record it in `checks.json`.

## Verification

- Run `python3 scripts/tests/run.py` (all control-plane tests including `test_comments.py`, `test_upgrade.py`, `test_oracle_pr_route.py`).
- Run `python3 -m unittest scripts/ops/test_retention.py scripts/ops/test_security_negative.py scripts/ops/test_compose_budget.py scripts/ops/test_check_new_comments.py -v`.
- Run `python3 scripts/repo.py status --change template-upgrade-f40b5e9`.
- Commit locally on branch `template-upgrade-f40b5e9` (no push while OCI 365-day backfill is active), then rebase `stage6-expected-decline-resume` onto `template-upgrade-f40b5e9`.
