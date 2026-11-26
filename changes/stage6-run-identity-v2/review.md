# Final Review — stage6-run-identity-v2

- **Reviewed implementation commit:** `8c59d1d2834dd967ea091156511344f4729c3f66` (base `e4eeab1` + review-fix commit `8c59d1d`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Neon (`gemini-4-neon-high`, read-only)

## Verification evidence

- `uv run --project generator --frozen pytest generator/tests`: 263 passed (`OK`).
- `uv run --project generator --frozen ruff check generator` & `ruff format --check generator`: passed (`50 files already formatted`).
- `uv run --project generator --frozen lint-imports --config generator/pyproject.toml --no-cache`: `Contracts: 2 kept, 0 broken`.
- `python3 scripts/ops/check_new_comments.py`: exit 0, no findings.
- `python3 scripts/repo.py status --change stage6-run-identity-v2 --base document-loopback-trust-topology`: `status: ok`, `readiness=ready`.
- **Deferred pre-merge gate:** Docker-dependent `python3 scripts/repo.py verify --change stage6-run-identity-v2 --full` is intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is active and must pass on the final PR head before merge.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Astra 6.2 | Important | `backfill_schedule_digest` filtered by `episode.window != "live"` instead of `at < backfill_end`, allowing live-D carry-in steps before `backfill_end` to bypass `_compatible_live_attach`. | **Fixed** in `8c59d1d`: filter by `at < backfill_end` + live-D carry-in rejection regression test. |
| Astra 6.2 + Neon | Important / Suggestion | Daily normal activity changes could shift a saved `wave_offset` without changing scenario-only digests; `schedule.profiles` was not hashed. | **Fixed** in `8c59d1d`: hashed `list(schedule.profiles.items())` into both digests, added `build_day_waves_digest`, persisted `wave_day_digest` with `wave_offset`, and added mid-day `normal=True` drift test. |
| Neon | Suggestion | `_compatible_live_attach` did not explicitly require v2 identity fields to be present. | **Fixed** in `8c59d1d`. |
| Astra 6.2 | Suggestion | `SALTATION_CODE_REVISION` was stored verbatim without format validation. | **Fixed** in `8c59d1d`: validated against safe revision pattern before claim. |

## Conclusion

All Important findings and Suggestions from both independent reviewers are resolved and verified locally. Ready for final PR-head `repo.py verify --full` once the active OCI 365-day backfill completes.
