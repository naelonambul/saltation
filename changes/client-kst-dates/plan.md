# Implementation Plan

## Summary

**Report.** During verification of `request-outcomes` (pull request #9) the host `e2e` run failed twice at about 02:00 KST on 2026-11-15: after signing in, the browser flow opened the first account and found "거래내역이 없습니다." although the balance (985,000원) showed that money had moved. The stage-1 smoke test passed the same day at 20:11 KST.

**Cause, read from the code.** The mobile client builds `from_date`, `to_date` and `tran_dtime` from the browser's local time (`web/apps/mobile/src/shared/ids.ts`, `tranDtime()` uses `getFullYear()`, `getHours()` and so on). The bank reads those dates as KST (`bank/src/features/accounts/mod.rs`, `parse_bound()`). The Playwright container runs in UTC, so between 00:00 and 09:00 KST its `to_date` is yesterday in Korea, and the history window ends before today's rows. A real customer whose phone is set to another time zone (travelling abroad, the benign lookalike of scenario G) sees the same: history missing the most recent day, and a `tran_dtime` off by hours. `request-outcomes` worked around it in the test harness by running the browser with `TZ=Asia/Seoul`; this change fixes the client.

**Fix.** The client formats every KFTC date and date-time in KST (UTC+9, no daylight saving), whatever the device's time zone. This is a `web/` change only; no contract, bank or generator change. The spec's "UI work starts after the API server is developed" concerns new screens (stage 7); this is a defect fix in a stage-1 screen's request code, so it does not wait for stage 7 unless the owner says otherwise.

Spec requirement concerned: F-38's history view (balances and history in the mobile web app), AC 8's "view balances and history".

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `web/apps/mobile/src/shared/ids.ts` | `tranDtime()` and `ymd()` format in KST from the instant's UTC time plus 9 hours, not from local fields |
| `web/apps/mobile/src/shared/ids.test.ts` | the existing date test builds its instants in UTC; new cases at both sides of 15:00 UTC (00:00 KST) |
| `web/apps/mobile/playwright.config.ts`, `web/apps/mobile/e2e/` | `timezoneId` from `E2E_TIMEZONE` (default `Asia/Seoul`) |
| `scripts/dev/it-oracle.sh` | `e2e` passes `E2E_TIMEZONE`; once the client is fixed, the browser no longer needs the `TZ=Asia/Seoul` that `request-outcomes` adds, and that line is removed (this branch is rebased onto `main` after #9 is merged) |

## Order of work

1. **Reproduce on the base.** Unit level: a test that formats `2026-11-14T17:38:00Z` and expects `20261115` and `20261115023800` fails on the base, run as `TZ=UTC pnpm --filter @saltation/mobile test` in `web/`. Whole product: `E2E_TIMEZONE=Etc/GMT+12 scripts/dev/it-oracle.sh e2e` on the OCI host, run before 21:00 KST (the browser's date then differs from Korea's), fails at the history step on the base with the history request's `to_date` one day behind; the request is read from the bank's `request.completed` event for that request ID. Both outputs are kept as evidence for the pull request.
2. **Fix** `ids.ts` and its tests.
   - Proof: `TZ=UTC`, `TZ=Asia/Seoul` and `TZ=America/Los_Angeles` runs of the mobile unit tests pass; `python3 scripts/repo.py verify --change client-kst-dates --group web`.
3. **Drive again.** `E2E_TIMEZONE=Etc/GMT+12 scripts/dev/it-oracle.sh e2e` passes, and so does the default (`Asia/Seoul`). The `TZ` line in `it-oracle.sh` is removed.
   - Proof: both runs, then `python3 scripts/repo.py verify --change client-kst-dates --full`.

## Tests and proof

The reproduction failing on the base and passing after the fix, in the unit tests and the `e2e` browser flow; the unit tests run under three time zones; the registered checks. The time-zone cases stay in `ids.test.ts` as the regression test.

## Risks and mitigations

- **The reproduction depends on the hour.** With the browser at UTC−12, its date differs from Korea's from 00:00 to 21:00 KST; the run is made in that window, and the unit tests reproduce at any hour.
- **Display of times.** The bank returns dates and times already in KST and the client shows them as text, so no display code changes; the unit tests for `format.ts` stay as they are.

## Rollback or recovery

Close the pull request before merge, or revert the squash commit after. No data changes.

## Open questions

- None.
