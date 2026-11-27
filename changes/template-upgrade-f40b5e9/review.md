# Final Review — template-upgrade-f40b5e9

- **Reviewed implementation commit:** `83abaa03c41746ab6f9f342f5d27821ca41b23c4` (base `9fbf184` + review-fix commit `83abaa0`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Carbon (`gemini-4-carbon-high`, read-only)

## Verification evidence

- `python3 scripts/tests/run.py`: 317 tests ran, 313 passed, 4 template-only skipped by pin (`OK`).
- `python3 -m unittest scripts/ops/test_check_new_comments.py scripts/tests/test_oracle_pr_route.py scripts/tests/test_comments.py -v`: 47 passed (`OK`).
- `python3 -m unittest scripts/ops/test_retention.py scripts/ops/test_security_negative.py scripts/ops/test_compose_budget.py -v`: 27 passed (`OK`).
- `python3 scripts/ops/check_new_comments.py`: exit 0, no findings.
- `python3 scripts/repo.py status --change template-upgrade-f40b5e9`: `status: ok`, `readiness=ready`, zero `template-drift`, zero `gates-undecided`.
- **Deferred pre-merge gate:** Full Docker/Oracle `python3 scripts/repo.py verify --change template-upgrade-f40b5e9 --full` is intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is running and must pass on the final PR head before merge.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Astra 6.2 | Important | `scripts/oracle_pr_route.py` did not allow `changes/<id>/review.md` in docs-only routing. | **Fixed** in `83abaa0` with unit tests in `scripts/tests/test_oracle_pr_route.py`. |
| Carbon | Important | `check_new_comments.py` diffed against `origin/main` tip rather than `git merge-base`. | **Fixed** in `83abaa0` with diverged-baseline regression test. |
| Carbon | Important | `check_new_comments.py` only inspected `ast.get_docstring`, missing other bare string statements and over-reporting delimiter-only reformatting. | **Fixed** in `83abaa0` using all `ast.Expr` string constants + delimiter normalization + tests. |
| Astra 6.2 | Important | `check_new_comments.py` missed inline comments moved to an adjacent statement in the same replacement block. | **Fixed** in `83abaa0` by matching comments per owning statement + adjacent-line tests. |
| Astra 6.2 | Important | Push CI on clean `main` compared `HEAD` against itself. | **Fixed** in `83abaa0` using `GITHUB_EVENT_BEFORE` / `HEAD~1` fallback + push tests. |
| Astra + Carbon | Suggestion | `comment-ratchet` runs via `scripts/tests/run.py` under `core` group rather than a standalone `python`-group check. | **Deferred** to avoid deviating from approved `plan.md`; can be split in a small follow-up if desired. |

## Conclusion

All five Important findings from both independent reviewers are resolved and verified locally. Ready for final PR-head `repo.py verify --full` once the active OCI 365-day backfill completes.
