# Stage 8 365-Day Backfill Interim Stability Report

> **OTN compliance notice (`REVIEW.md`).** This report records operational stability observations, container health, point-in-time memory and storage headroom, ledger and outbox aggregate counts, scenario credit progress, and the generator-side balance-depletion root cause observed during the run. Oracle-backed throughput, latency, wait-time, execution-plan, wall-clock run duration, and pacing/extrapolation measurements are excluded and kept only in ignored local `.evidence/`.

## 1. Run Overview

| Field | Value |
| --- | --- |
| **Host** | Primary OCI Ampere A1 ARM64 development host (4 vCPU, 24 GB RAM) |
| **Revision** | `93fd480` (`main`, after PR #62) |
| **Run ID** | `oci-365d-live-93fd480` |
| **Argon2id Configuration (Spec N-6)** | OWASP parameter profile (`SALTATION_ARGON2_M_KIB=19456`, `SALTATION_ARGON2_T_COST=2`, `SALTATION_ARGON2_P_COST=1`); Saltation semaphore cap `SALTATION_ARGON2_CONCURRENCY=3` |
| **Launched Command** | `generator run-backfill-live --run-id oci-365d-live-93fd480 --first 2025-11-28 --days 365 --live-at 2026-11-28T11:00:00+09:00 --live-seconds 86400` |
| **Business-Date Coverage Reached** | `2025-11-28` through partial day 249 (`2026-08-03T10:01:15+09:00`); `rolled_through = 2026-08-02` (**248 / 365 completed backfill days**, `67.9%`) |
| **Outcome** | **Interim stop (generator-side abort)** on `annual-A-121/transfer` (`422` / `405 잔액이 부족합니다`); 24-hour live phase was **not entered** |

## 2. Post-Abort Container & Host Snapshot

At post-abort inspection, all eight persistent Saltation Compose services were running with **zero restarts** and **zero OOM kills**, and the detached `saltation-365d-live` workload driver container had exited cleanly with code `1` (`OOMKilled=false`, `restarts=0`). Memory values below are **post-abort point-in-time snapshots**, not continuous 248-day peaks:

| Container | Post-Abort Status | OOMKilled | Restarts | Snapshot Memory / Limit |
| --- | --- | --- | --- | --- |
| `saltation-bank-1` | `running (healthy)` | `false` | `0` | `112.7 MiB / 1 GiB` (`11.01%`) |
| `saltation-oracle-1` | `running (healthy)` | `false` | `0` | `4.458 GiB / 6 GiB` (`74.29%`) |
| `saltation-mongo-1` | `running (healthy)` | `false` | `0` | `1.699 GiB / 3 GiB` (`56.64%`) |
| `saltation-web-1` | `running (healthy)` | `false` | `0` | `5.32 MiB / 128 MiB` (`4.16%`) |
| `saltation-prometheus-1` | `running (healthy)` | `false` | `0` | `59.93 MiB / 768 MiB` (`7.80%`) |
| `saltation-grafana-1` | `running (healthy)` | `false` | `0` | `136.2 MiB / 256 MiB` (`53.22%`) |
| `saltation-loki-1` | `running (healthy)` | `false` | `0` | `94.7 MiB / 768 MiB` (`12.33%`) |
| `saltation-alloy-1` | `running` | `false` | `0` | `65.09 MiB / 256 MiB` (`25.42%`) |
| `saltation-365d-live` (driver) | `exited (1)` | `false` | `0` | `n/a` (exited) |

- **`saltation-bank-1` post-abort process memory gauges (`/metrics`):**
  - `bank_process_resident_memory_bytes`: `84,860,928` (**80.9 MiB** / 1 GiB limit)
  - `bank_process_anon_memory_bytes`: `73,945,088` (**70.5 MiB**)
- **Post-abort readiness (`/readyz`):** `{"status":"ok","oracle":"ok"}`
- **Host headroom at post-abort snapshot:** `15 GiB` RAM available (`7.7 GiB / 23 GiB` used, `0 B` swap); whole host block volume `/home/ckh/block` at `67G / 147G` (**48%**, `73G` free — total host volume usage, not isolated to Saltation-only N-9 storage); root filesystem `/` at `15G / 49G` (**31%**, `34G` free).

## 3. Ledger, Outbox & Truth Observations (Through Abort on Day 249)

The counts below were observed at post-abort inspection covering dormant preparation, 248 completed backfill dates (`2025-11-28` through `2026-08-02`), and partial day 249 up to `2026-08-03T10:01:15+09:00`:

- **HTTP 5xx responses:** **0**
- **Outbox relay errors (`bank_outbox_relay_errors_total`):** **0**
- **Outbox backlog (`bank_outbox_undelivered`):** **0**
- **Aggregate outbox and log counts:** `bank_outbox_delivered_total = 9,671,720` and `bank_logs.events` estimated document count `= 9,671,720` (matching aggregate totals with an empty outbox queue; full per-request-to-event identity reconciliation per Spec N-5 was not run on this aborted run)
- **Daily close (`bank_daily_close_runs_total = 614`):** `366` dormant-cohort preparation rollover dates (Spec F-11, `stage6.py:492` / `stage6_seed.py:162-166`) plus `248` completed backfill dates (`2025-11-28` through `2026-08-02`). At inspection, the last-close gauge reported `bank_daily_close_mismatches = 0` and zero daily-close mismatch log entries appeared in `saltation-bank-1` container logs.
- **Ground-truth persistence (`truth` database):**
  - `roster`: `20,000` customers
  - `labels`: `6,633,784` persisted request labels
  - `stage6_operations` (**retained/unpruned** documents only — completed `normal_*` rows older than 7 days are pruned into `truth.runs.archived_normal` per `stage6.py:318,662-664`): `175,098` retained documents (`174,935` `succeeded`, `163` `declined`). Breakdown of the `163` retained `declined` rows:
    - `143` expected Scenario G `declined_card` steps (`expect_declined=True`)
    - `19` permitted `normal_*` declines within the trailing 7-day unpruned window (`13` `normal_visit/transfer` `405`, `4` `normal_card`, `2` `normal_transfer/transfer` `405`)
    - `1` aborting required step (`annual-A-121/transfer`, `405`)
  - Credited Stage 6 records (`stage6_credit` — **2,286 credit records** through the abort point on partial day 249; note that each `BC` episode credits its `B` victim(s) and `C` mule separately):

| Kind | Credit Records |
| --- | --- |
| `A` | `122` |
| `B` | `128` |
| `C` | `64` |
| `D` | `66` |
| `E` | `208` |
| `F` | `209` |
| `G` | `143` |
| `lookalike` | `1,346` |

## 4. Abort Root Cause Analysis (`annual-A-121/transfer`)

The workload driver exited (`exit=1`) with:

```text
required operation declined for annual-A-121/transfer
```

Inspection of `truth.stage6_operations`, `truth.roster`, and `bank_logs.events` confirmed this was **not a bank or infrastructure defect**, but a **generator-side long-horizon balance-depletion bug**:

- **Subject:** `C00011143` (`persona = student`, account `99800011143`, `opening_balance = 3,000,000` KRW; ring predecessor was non-dormant `working` customer `C00009359`).
- **Accumulated normal activity prior to `2026-08-03`:**
  - `INBOUND` (`settlement`, 8 months): `+3,600,000` KRW (plus incoming normal ring transfers)
  - `TRANSFER` (`43` completed normal outflows, max single draw `1,199,413` KRW; plus an earlier `2026-06-21` normal transfer of `1,442,829` KRW legitimately rejected with `405`)
  - `CARD_PAYMENT` (`67` domestic approvals): `-1,402,363` KRW
  - `ATM_WITHDRAW` (`9` withdrawals): `-270,000` KRW
- **Failure mechanism:** At `93fd480`, `generator/saltation_gen/drivers/stage6_seed.py:77` seeds all non-dormant customers with an inline `3,000,000` KRW opening balance, while `normal_transfer` / `normal_visit` draws from `BANKING_KRW_ANCHORS` (mean `147,250` KRW, maximum `2,000,000` KRW). Over 248+ days, `student` customers (`450,000` KRW/month settlement — and more generally any `student` whose ring predecessor is a dormant `C` mule and falls back to `older`) can random-walk below Scenario A's fixed `1,500,000` KRW debit. When `annual-A-121/transfer` requested `1,500,000` KRW on `2026-08-03T10:01:15+09:00`, the bank correctly enforced F-19/F-22 and returned HTTP `422` (`rsp_code: A0004`, `bank_rsp_code: 405`, `잔액이 부족합니다`), causing the Stage 6 executor to abort on a required scenario operation.
- **Why in-place resume is not valid:** `annual-A-121/transfer` is durably recorded as terminally declined in `truth.stage6_operations`, so resuming `oci-365d-live-93fd480` repeats the same abort immediately. Manual balance injection would break reproducibility and leave other `student` victims vulnerable across the remaining 117 days.

## 5. What Remains Unverified

Because this run aborted on day 249 of 365 before entering live mode, the following acceptance targets remain **unverified** and require a fresh end-to-end run after remediation:

- Full 365-day backfill completion and N-4 pacing window
- Remaining backfill dates 249–365 (including F-11 dormancy rollovers on normal accounts and full per-date `DAILY_CLOSE` + N-5 request-to-event reconciliation)
- Full 365-day isolated Saltation volume footprint against the N-9 70 GB budget
- Phase-wide N-7/N-8 qualification (continuous host watcher, effective quotas, and driver inventory across both backfill and live phases)
- The entire 24-hour live phase (cutover, live Scenario D carry-in, and live scenario quotas)

## 6. Proposed Follow-Up Steps

1. **Fix Stage 6 365-day balance headroom (proposed `stage6-opening-balance-headroom` change):**
   - Replace the inline `3,000,000` KRW non-dormant opening balance in `stage6_seed.py:77` with a uniform `20,000,000` KRW opening balance across all non-dormant customers (preserving uniform non-dormant seeding; to be validated with multi-seed 365-day balance replay tests in that packet).
   - Raise Scenario G `declined_card` from USD `50,000` (`65,000,000` KRW) to a higher F-10-compliant threshold (proposed USD `200,000` = `260,000,000` KRW) and validate against scheduled balances including the highest-income persona (`sole_trader`, `44,200,000` KRW/year) plus incoming ring transfers.
   - Bump workload compatibility version so previously seeded `3,000,000`-KRW worlds cannot silently resume under the updated funding semantics (conditional on merging the queued `stage6-run-identity-v2` change that introduces `WORKLOAD_VERSION = 2` on top of `main`'s `"version": 1` manifest).
2. **Re-run full 365-day backfill + 24-hour live qualification:** After merging the queued hardening PRs and the balance-headroom fix, perform a clean `down -v && up -d --wait` and execute the full 365-day backfill through the 24-hour live window.
