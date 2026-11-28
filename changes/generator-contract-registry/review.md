# Final Review — generator-contract-registry

- **Reviewed implementation commit:** `4d355f010e38ebc5a189f58dbb7c64519e2f4bb1` (base `8da378c` + review-fix commit `4d355f0`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Neon (`gemini-4-neon-high`, read-only)

## Verification evidence

- `uv run --project generator --frozen pytest generator/tests -q`: 308 passed (`OK`).
- `uv run --project generator --frozen ruff check generator` & `ruff format --check generator`: passed (`52 files already formatted`).
- `uv run --project generator --frozen lint-imports --config generator/pyproject.toml --no-cache`: `Contracts: 2 kept, 0 broken`.
- `python3 scripts/ops/check_new_comments.py`: exit 0, no findings.
- `bash -n scripts/dev/ci-py.sh`: exit 0.
- `python3 scripts/repo.py status --change generator-contract-registry --base stage6-run-identity-v2`: `status: ok`, `readiness=ready`.
- **Deferred pre-merge gate:** Docker-dependent `python3 scripts/repo.py verify --change generator-contract-registry --full` (including `py-test` inside `scripts/dev/ci-py.sh` with the `-v "$root/contracts":/contracts:ro` mount) is intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is active and must pass on the final PR head before merge.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Astra 6.2 | Important | In `extract_paths`, a nested key inside a multiline flow mapping under `responses:` (e.g. `x-metadata: {\n "404": "nested"\n }`) could satisfy a missing response status. | **Fixed** in `4d355f0`: added `scan_flow_delta()` flow-depth tracking so lines inside open flow collections are never parsed as block-level keys, plus negative regression test `test_extractor_ignores_nested_status_in_multiline_flow_mapping`. |
| Astra 6.2 + Neon | Important / Note | Local `repo.py verify --full` evidence is not yet recorded for this revision. | **Deferred to PR CI**: local Mac has no Docker daemon and OCI runner is reserved for the active 365-day backfill; full `repo.py verify --full` will run before merge. |
| Neon | Minor | `extract_paths` did not verify `:` presence before matching keys and checked multiline scalars before the `indent == 0` exit of `paths:`. | **Fixed** in `4d355f0`: `indent == 0` exit runs first and `entry.partition(":")` now requires a separator before key matching. |
| Astra 6.2 | Minor | `extract_paths` assumes standard 2-space OpenAPI indentation. | **Accepted**: all repository OpenAPI specs use 2-space indentation; strict indentation keeps the stdlib parser minimal and deterministic. |
| Neon | Minor | `generator/saltation_gen/drivers/behavior.py` still defines two endpoint constants outside `OPERATIONS`; `operation_path()` ignores extra kwargs. | **Out of scope**: `behavior.py` is outside this packet's `write_scope` and can be migrated in a follow-up if needed. |

## Conclusion

All Important code findings from both independent reviewers are resolved and verified locally (`308 passed`). Ready for final PR-head `repo.py verify --full` once the active OCI 365-day backfill completes.
