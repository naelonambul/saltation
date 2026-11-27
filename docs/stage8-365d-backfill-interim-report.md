# Stage 8 365-Day Backfill Interim Stability Report

> **OTN compliance notice (`REVIEW.md`).** This report records operational stability, container health, memory and storage headroom, ledger and outbox correctness, scenario delivery progress, and the generator-side balance-depletion root cause observed during the run. Oracle-backed throughput, latency, wait-time, execution-plan, wall-clock run duration, and pacing/extrapolation measurements are excluded and kept only in ignored local `.evidence/`.

## 1. Run Overview

| Field | Value |
| --- | --- |
| **Host** | Primary OCI Ampere A1 ARM64 development host (4 vCPU, 24 GB RAM) |
| **Revision** | `93fd480` (`main`, after PR #62) |
| **Run ID** | `oci-365d-live-93fd480` |
| **Argon2id Profile (Spec N-6)** | OWASP default (`m_kib=19456`, `t_cost=2`, `p_cost=1`, `concurrency=2`) |
| **Workload** | `generator run-backfill-live --run-id oci-365d-live-93fd480 --first 2025-11-28 --days 365 --live-at 2026-11-28T11:00:00+09:00 --live-seconds 86400` |
| **Business-Date Coverage Reached** | `2025-11-28` through `2026-08-03T10:01:15+09:00` (`rolled_through = 2026-08-02`, **248 / 365 business days**, `68.0%`) |
| **Outcome** | **Interim stop (generator-side abort)** on `annual-A-121/transfer` (`422` / `405 잔액이 부족합니다`); 24-hour live phase was **not entered** |

## 2. Container Health & Resource Bounds (Spec N-7 / N-8 / N-9)

All eight Saltation Compose services ran continuously through the entire 248-business-day backfill with **zero restarts** and **zero OOM kills**:

| Container | Status | OOMKilled | Restarts | Observed Memory / Limit |
| --- | --- | --- | --- | --- |
| `saltation-bank-1` | `running (healthy)` | `false` | `0` | `112.7 MiB / 1 GiB` (`11.01%`) |
| `saltation-oracle-1` | `running (healthy)` | `false` | `0` | `4.458 GiB / 6 GiB` (`74.29%`) |
| `saltation-mongo-1` | `running (healthy)` | `false` | `0` | `1.699 GiB / 3 GiB` (`56.64%`) |
| `saltation-web-1` | `running (healthy)` | `false` | `0` | `5.32 MiB / 128 MiB` (`4.16%`) |
| `saltation-prometheus-1` | `running (healthy)` | `false` | `0` | `59.93 MiB / 768 MiB` (`7.80%`) |
| `saltation-grafana-1` | `running (healthy)` | `false` | `0` | `136.2 MiB / 256 MiB` (`53.22%`) |
| `saltation-loki-1` | `running (healthy)` | `false` | `0` | `94.7 MiB / 768 MiB` (`12.33%`) |
| `saltation-alloy-1` | `running` | `false` | `0` | `65.09 MiB / 256 MiB` (`25.42%`) |

- **`saltation-bank-1` process memory gauges (`/metrics`):**
  - `bank_process_resident_memory_bytes`: `84,860,928` (**80.9 MiB** / 1 GiB limit)
  - `bank_process_anon_memory_bytes`: `73,945,088` (**70.5 MiB**)
- **Readiness (`/readyz`):** `{"status":"ok","oracle":"ok"}`
- **Host headroom:** `15 GiB` RAM available (`7.7 GiB / 23 GiB` used, `0 B` swap); block volume `/home/ckh/block` at `67G / 147G` (**48%**, `73G` free); root filesystem `/` at `15G / 49G` (**31%**, `34G` free).

## 3. Ledger, Outbox & Truth Correctness (Spec F-23 / F-35 / F-36a / N-5)

Across all 248 completed business days (before the generator-side abort):

- **HTTP 5xx responses:** **0**
- **Outbox relay errors (`bank_outbox_relay_errors_total`):** **0**
- **Outbox backlog (`bank_outbox_undelivered`):** **0** (`bank_outbox_oldest_age_seconds = 0`)
- **Outbox delivery completeness:** `bank_outbox_delivered_total = 9,671,720`, matching `bank_logs.events` (`9,671,720` documents) 1:1 with zero dropped events
- **Daily close (`bank_daily_close_mismatches`):** **0 mismatches** across `614` daily-close runs (`bank_daily_close_runs_total = 614`)
- **Ground-truth persistence (`truth` database):**
  - `roster`: `20,000` customers
  - `labels`: `6,633,784` persisted request labels
  - `stage6_operations`: `175,098` recorded operations (`174,935` `succeeded`, `163` `declined`)
  - Credited Stage 6 episodes (`stage6_credit` — `2,286` total through day 248):

| Kind | Credited Episodes |
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

The run exited cleanly (`exit=1`) with:

```text
required operation declined for annual-A-121/transfer
```

Inspection of `truth.stage6_operations`, `truth.roster`, and `bank_logs.events` confirmed this was **not a bank or infrastructure defect**, but a **generator-side long-horizon balance-depletion bug**:

- **Subject:** `C00011143` (`persona = student`, account `99800011143`, `opening_balance = 3,000,000` KRW).
- **Accumulated normal activity prior to `2026-08-03`:**
  - `INBOUND` (`settlement`, 8 months): `+3,600,000` KRW (plus incoming normal ring transfers)
  - `TRANSFER` (43 completed normal outflows, max single draw `1,199,413` KRW; plus an earlier `2026-06-21` normal transfer of `1,442,829` KRW legitimately rejected with `405`)
  - `CARD_PAYMENT` (67 domestic approvals): `-1,402,363` KRW
  - `ATM_WITHDRAW` (9 withdrawals): `-270,000` KRW
- **Failure mechanism:** `generator/saltation_gen/drivers/stage6_seed.py` seeds all non-dormant customers with a flat `3,000,000` KRW opening balance, while `normal_transfer` / `normal_visit` draws from `BANKING_KRW_ANCHORS` (mean `147,250` KRW, maximum `2,000,000` KRW). Over 248+ days, `student` customers (`450,000` KRW/month settlement, especially when their ring predecessor is a dormant `C` mule and falls back to `older`) can random-walk below Scenario A's fixed `1,500,000` KRW debit. When `annual-A-121/transfer` requested `1,500,000` KRW on `2026-08-03T10:01:15+09:00`, the bank correctly enforced F-19/F-22 and returned HTTP `422` (`rsp_code: A0004`, `bank_rsp_code: 405`, `잔액이 부족합니다`), causing the Stage 6 executor to abort on a required scenario operation.
- **Why in-place resume is not valid:** `annual-A-121/transfer` is durably recorded as terminally declined in `truth.stage6_operations`, so resuming `oci-365d-live-93fd480` repeats the same abort immediately. Manual balance injection would break reproducibility and leave other `student` victims vulnerable across the remaining 117 days.

## 5. Planned Next Steps

1. **Fix Stage 6 365-day balance headroom (`stage6-opening-balance-headroom`):**
   - Raise the uniform non-dormant `STAGE6_OPENING_BALANCE` from `3,000,000` KRW to `20,000,000` KRW (verified across 50,000 Monte Carlo trials with zero `student` depletions below `1,500,000` KRW, preserving uniform non-dormant seeding).
   - Raise Scenario G `declined_card` from USD `50,000` (`65,000,000` KRW) to USD `200,000` (`260,000,000` KRW, within F-10) so even the highest-income persona (`sole_trader`, `44,200,000` KRW/year) remains strictly below the expected-decline threshold across 365 days + 24 hours live.
   - Bump `WORKLOAD_VERSION` from `2` to `3` so previously seeded `3,000,000`-KRW worlds cannot silently resume under the updated funding semantics.
2. **Re-run full 365-day backfill + 24-hour live qualification:** After merging the queued hardening PRs and the balance-headroom fix, perform a clean `down -v && up -d --wait` and execute the full 365-day backfill through the 24-hour live window.
