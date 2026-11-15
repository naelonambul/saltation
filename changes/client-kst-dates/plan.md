# Implementation Plan

## Summary

**Report.** During verification of `request-outcomes` (pull request #9) the host `e2e` run failed twice at about 02:00 KST on 2026-11-15: after signing in, the browser flow opened the first account and found "거래내역이 없습니다." although the balance (985,000원) showed that money had moved. The stage-1 smoke test passed the same day at 20:11 KST.

**Cause, read from the code.** The mobile client builds `from_date`, `to_date` and `tran_dtime` from the browser's local time (`web/apps/mobile/src/shared/ids.ts`, `tranDtime()` uses `getFullYear()`, `getHours()` and so on). The bank reads those dates as KST (`bank/src/features/accounts/mod.rs`, `parse_bound()`). The Playwright container runs in UTC, so between 00:00 and 09:00 KST its `to_date` is yesterday in Korea, and the history window ends before today's rows. A real customer whose phone is set to another time zone (travelling abroad, the benign lookalike of scenario G) sees the same: history missing the most recent day, and a `tran_dtime` off by hours. `request-outcomes` worked around it in the test harness by running the browser with `TZ=Asia/Seoul`; this change fixes the client.

**Fix.** The client formats every KFTC date and date-time in KST (UTC+9, no daylight saving), whatever the device's time zone. The client code change is in `web/` only; no contract, bank or generator change. The browser reproduction becomes a registered check (below). The spec says the stage-1 mobile flow stays as it is until stage 7 (`spec.md`, Delivery); this fix changes the dates that flow sends, not its screens or contract use, but it is still a change to that flow; the owner decided on 2026-11-15 to fix it now rather than hold it until stage 7.

Spec requirement concerned: F-38's history view (balances and history in the mobile web app), AC 8's "view balances and history".

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `web/apps/mobile/src/shared/ids.ts` | `tranDtime()` and `ymd()` format in KST from the instant's UTC time plus 9 hours, not from local fields |
| `web/apps/mobile/src/shared/ids.test.ts` | the existing date test builds its instants in UTC; new cases at both sides of 15:00 UTC (00:00 KST), each run with `process.env.TZ` set in turn to `UTC`, `Asia/Seoul` and `America/Los_Angeles` inside the test (Node applies a changed `TZ` at run time), so the registered `web-test` check covers the three zones without a script change |
| `web/apps/mobile/playwright.config.ts`, `web/apps/mobile/e2e/smoke.spec.ts` | `timezoneId` from `E2E_TIMEZONE` when it is set, otherwise the container's own zone (UTC); the smoke test records and prints the history request's `from_date` and `to_date` and checks `to_date` is Korea's date; after its transfer it opens the account again and checks the history lists that transfer (today's row), so a window ending yesterday fails even when other rows exist |
| `checks.json` | a new check `mobile-browser-dates` in group `oracle`, running `scripts/dev/it-oracle.sh browser-dates`, routed from `web/apps/mobile/**`, `bank/src/features/accounts/**` and `scripts/dev/it-oracle.sh` |
| `scripts/dev/it-oracle.sh` | a new target `browser-dates`: brings the stack up, seeds counterparties, switches the clock to live, seeds two customers, and runs the Playwright smoke test twice, with the browser in `Etc/GMT+12` and in `Pacific/Kiritimati` (UTC+14); at every hour of the day one of the two has a date different from Korea's (UTC−12 from 00:00 to 21:00 KST, UTC+14 from 19:00 to 24:00 KST), so the check reproduces at any hour. `e2e` passes `E2E_TIMEZONE`; once the client is fixed, the browser no longer needs the `TZ=Asia/Seoul` that `request-outcomes` adds, and that line is removed (`main` has it since #9) |

## Order of work

1. **Reproduce on the base.** With the smoke-test and harness changes above but the client unchanged, `scripts/dev/it-oracle.sh browser-dates` fails on the OCI host at any hour, in the zone whose date differs from Korea's, at the history check, printing a `to_date` one day off. Unit level: a test that formats `2026-11-14T17:38:00Z` and expects `20261115` and `20261115023800` fails on the base, run as `TZ=UTC pnpm --filter @saltation/mobile test` in `web/`. Whole product: `E2E_TIMEZONE=Etc/GMT+12 scripts/dev/it-oracle.sh e2e` on the OCI host, run before 21:00 KST (the browser's date then differs from Korea's), fails at the history step on the base; the smoke test records the history request's URL (`page.on("request")`) and prints its `to_date`, one day behind Korea's date. The bank's events do not keep query strings, so the browser side is the evidence. Both outputs are kept as evidence for the pull request.
2. **Fix** `ids.ts` and its tests.
   - Proof: the mobile unit tests pass, including the three-zone cases; `python3 scripts/repo.py verify --change client-kst-dates --group web`.
3. **Drive again.** The `TZ` line in `it-oracle.sh` is removed, so the default `e2e` browser runs in UTC, not in Korean time. `E2E_TIMEZONE=Etc/GMT+12 scripts/dev/it-oracle.sh e2e` passes and prints a `to_date` equal to Korea's date, and so does the default run.
   - Proof: both runs, `scripts/dev/it-oracle.sh browser-dates` passing, then `python3 scripts/repo.py verify --change client-kst-dates --full`, which runs the new check.

## Tests and proof

The reproduction failing on the base and passing after the fix, in the unit tests and the `e2e` browser flow; the unit tests run under three time zones; the registered checks. The regression tests are the time-zone cases in `ids.test.ts` and the registered `mobile-browser-dates` check, which drives the real browser flow in two zones.

## Risks and mitigations

- **The reproduction depends on the hour.** One zone alone differs from Korea's date only part of the day; `browser-dates` runs two zones that together cover every hour, and the `E2E_TIMEZONE=Etc/GMT+12` run is made before 21:00 KST.
- **CI time.** `browser-dates` adds one stack start and two browser runs (a few minutes) to the `oracle` group on the OCI runner.
- **Display of times.** The bank returns dates and times already in KST and the client shows them as text, so no display code changes; the unit tests for `format.ts` stay as they are.

## Rollback or recovery

Close the pull request before merge, or revert the squash commit after. No data changes.

## Open questions

- None.
