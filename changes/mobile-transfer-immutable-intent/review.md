# Final Review — mobile-transfer-immutable-intent

- **Reviewed implementation commit:** `e0d40cd12805155720ce0a6635d67432b216f9c4` (base `e68afe6` + review-fix commit `e0d40cd`)
- **Reviewers:** Codex `gpt-6.2-astra` (`medium`, read-only) and AGY Neon (`gemini-4-neon-high`, read-only)

## Verification evidence

- `vitest run` (`@saltation/mobile`): 2 test files, 55 passed (`OK`).
- `tsc -p tsconfig.json` (`@saltation/mobile`): exit 0, no diagnostics.
- `vite build` (`@saltation/mobile`): exit 0, 34 modules transformed.
- `eslint .` (`web`): exit 0, no diagnostics.
- `python3 scripts/repo.py status --change mobile-transfer-immutable-intent --base stage6-expected-decline-resume`: `status: ok`, `readiness=ready`.
- **Deferred pre-merge gate:** Docker-backed `python3 scripts/repo.py verify --change mobile-transfer-immutable-intent --full` and Playwright browser E2E are intentionally deferred while the OCI 365-day backfill (`oci-365d-live-93fd480`) is active and must pass on the final PR head before merge.

## Findings and disposition

| Source | Severity | Finding | Disposition |
|---|---|---|---|
| Astra 6.2 + Neon | Important | Empty/malformed `2xx` (`200` empty, `204`, `{}`) or unexpected `< 400` responses failed open as `rejected` or false-completed. | **Fixed** in `e0d40cd`: fail-closed `classifyTransferStatus` (only `400..499` excluding `408`/`429` is `rejected`) + strict string payload check + tests. |
| Astra 6.2 | Important | After a prior ambiguous attempt, a retry returning `401` (expired session), `403`, or `409` cleared the original `bankTranId`. | **Fixed** in `e0d40cd`: `priorAmbiguous` preserves `intent` and `bankTranId` across `401`/`403`/`409` retries + tests. |
| Neon | Important | Hiding `onBack` during `ambiguous` blocked navigation back to Home to check balance/history. | **Fixed** in `e0d40cd`: hide `onBack` only while `busy`, confirm before leaving during `ambiguous`. |
| Astra 6.2 + Neon | Suggestion/Nit | Missing `보내는 중…` label while busy; missing idempotent-retry notice on ambiguous HTTP errors; `inFlight` race guards and `finally` cleanup. | **Fixed** in `e0d40cd`. |

## Conclusion

All Important findings and Suggestions from both independent reviewers are resolved and verified locally. Ready for final PR-head `repo.py verify --full` and browser E2E once the active OCI 365-day backfill completes.
