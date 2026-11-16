# Integration test timing

## Baseline — 2026-11-15

Mac Colima, original oracle 0.6.3 driver, development profile. One full `scripts/dev/it-oracle.sh all` run passed, including observability. These are single-run suite durations, not medians or CI results. Source: `.evidence/codex-resume-20261115/baseline-all.log`.

| Suite | First pass (s) | Second pass (s) |
|---|---:|---:|
| atm | 49.29 | 49.90 |
| outcomes | 9.37 | 9.93 |
| password_reset | 11.45 | 11.11 |
| transfers | 13.72 | 14.57 |
| backfill | 36.08 | — |

The first attempt failed during parallel linking (signal 9). Prebuilding with `cargo test --locked --tests --no-run -j 2` succeeded, then the full run passed. No assertions were changed.

## Remaining measurements

The completed measurements and verification are recorded below. CI job comparison awaits a published branch; OCI local verification is measured separately.

## Two hashing packages optimized

`argon2` and `blake2` use dev `opt-level = 3`; bank settings and password parameters are unchanged. Full `it-oracle.sh all` passed, including observability, with identical test counts. Single-run durations (first / second pass):

| Suite | Seconds |
|---|---:|
| ATM (30 tests) | 7.13 / 8.94 |
| Outcomes (2 tests) | 1.49 / 1.96 |
| Password reset (1 test) | 1.05 / 1.05 |
| Transfers (5 tests) | 2.61 / 2.41 |
| Backfill (1 test) | 19.94 |

Evidence: `.evidence/codex-resume-20261115/two-package-all.log`. These are observations from one run, not repeated medians.

## Wildcard experiment

A clean-cache `cargo --config 'profile.dev.package."*".opt-level=3' build --locked --tests --timings -j 2` failed during linking with `No space left on device` in Colima. The experiment-only volume was removed; existing volumes were preserved. No valid warm comparison or complete cold-build duration was obtained, so the wildcard setting is not adopted. Evidence: `.evidence/codex-resume-20261115/wildcard-build.log`. The owner subsequently authorized disk expansion. Colima was expanded from 100 to 200 GiB, leaving 97 GiB available; the comparison will be retried.

## Validator cache

Parsed contracts and compiled validators are cached per process with directory/file/schema keys. Each response is validated anew. The invalid-body regression and full `it-oracle.sh all` passed, including observability. Evidence: `.evidence/codex-resume-20261115/cache-all.log`; independent Astra review found no concrete issues.

## Isolated secrecy scans

The two secrecy tests capture the OUTBOX ULID watermark before `live()` and require their own session events in the scan. Seeding itself emits no OUTBOX event; its test now starts and ends a session on the newly seeded account to prove a nonempty scan while retaining response, log, PIN, token and hash checks. `it-oracle.sh atm` passed twice without reset: 31 tests each, 7.83 s and 7.28 s. The added test is the cached-validator regression. Evidence: `.evidence/codex-resume-20261115/atm-repeat-{1,2}.log`.

## Wildcard retry after disk expansion

A frozen pre-cache source copy built successfully with all dependency optimization in a fresh volume: 4m50s. Its full `all` run passed. Summed Cargo test durations were 39.97 s versus 56.77 s for the two-package run: 16.80 s additional saving, below the 30 s adoption threshold. Retain only argon2/blake2 overrides. This comparison is one run each, not a median or end-to-end CI duration.

## Connection churn

After caching, the entire two-pass Cargo test time was 39.22 s. The ATM suite was below 10 s per pass before duplicate removal, so `common::state()` cannot exceed the 10 s per-pass threshold. Smaller test pool defaults are not needed; defaults remain unchanged.

## Final measurements and verification

Mac summed Cargo suite durations over three successful full runs:

| State | Runs (s) | Median (s) |
|---|---|---:|
| Baseline | 248.98, 243.70, 248.48 | 248.48 |
| Final | 35.11, 33.12, 33.70 | 33.70 |

This is an 86.4% reduction in summed test time, not wall-clock CI time. Baseline runs 2–3 include per-test `--report-time` output. Final runs include one ATM pass, all original assertions, and the negative validator regression. The live-mode precondition passed.

On OCI, `python3 scripts/repo.py verify --change test-speed --full` passed all 13 checks, complete=true; Oracle integration took 349.824 s (5m50s), including build/harness work. Evidence: `.evidence/20261115T150916766547Z/evidence.json`. This is a local OCI run, not a GitHub CI job. Mac default Python3.9 failed the control-plane test import; Python3.13 passed all 213 control-plane tests, and OCI full verification passed.

Astra final review reported no concrete issues. Hydrogen findings were checked against source and the approved plan (disposition in `.evidence/codex-resume-20261115/review-disposition.txt`); Argon was unavailable due weekly quota. No assertions or password parameters were relaxed.
