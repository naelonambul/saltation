# Stage 8 24-Hour Continuous Stability Report

> **OTN compliance notice (`REVIEW.md`).** This report records operational stability, memory bounds, container health, ledger and outbox correctness, and follow-up engineering findings. Oracle-backed throughput, latency, wait-time, execution-plan, and benchmark-derived pacing and duration measurements are excluded and kept only in ignored local `.evidence/`.

## 1. Run Overview

| Field | Value |
| --- | --- |
| **Host** | Primary OCI Ampere A1 ARM64 development host (4 vCPU, 24 GB RAM) |
| **Revision** | `510c6c9` (`main`, after PR #35–#42) |
| **Run ID** | `stage8-fullrun-20261123` |
| **Window (UTC)** | `2026-11-23T01:57:06Z` – `2026-11-24T01:59:29Z` |
| **Workload** | `generator run-backfill-live --first 2025-11-23 --days 365 --live-at 2026-11-23T00:00:00+09:00 --live-seconds 86400 --customers 20000 --amplification 20 --workers 8` |
| **Monitoring** | 60-second host watcher (`1,440` samples), `/readyz` probe, Prometheus `/metrics` RSS gauges (`bank_process_resident_memory_bytes`, `bank_process_anon_memory_bytes`), `SaltationBankMemoryHigh` alert |

## 2. Prior Incident Resolution (PR #38 Verification)

Before PR #38 (`bank-argon2-oom`), an earlier Stage 8 full run on the same OCI host was OOM-killed (`OOMKilled=true`, `ExitCode=137` under the 1 GiB `saltation-bank-1` limit) due to glibc `ptmalloc2` arena retention of 19 MiB Argon2id buffers across Tokio `spawn_blocking` threads.

With PR #38 (`GLIBC_TUNABLES=glibc.malloc.arena_max=2:glibc.malloc.mmap_threshold=131072` + `OwnedSemaphorePermit` bound to the blocking closure) and PR #41 (process RSS gauges) deployed:

- **24-hour peak `saltation-bank-1` `VmRSS`**: **63.5 MiB / 1 GiB** (6.2% of container limit)
- **24-hour peak `saltation-bank-1` `RssAnon`**: **52.0 MiB**
- **Steady-state behavior at hour 24**: `RssAnon` oscillated cleanly between ~13 MiB (idle) and ~51 MiB (two concurrent 19 MiB Argon2id verifications), immediately releasing memory back to the OS with **zero monotonic growth**.
- **Watcher alerts (`/tmp/saltation-watch-alert.log`)**: **0**

## 3. Container & Co-Tenant Safety (Spec N-8)

All 10 active containers ran continuously across the 24-hour window with zero OOM kills and zero restarts:

| Container | Status | OOMKilled | Restarts |
| --- | --- | --- | --- |
| `saltation-bank-1` | `running (healthy)` | `false` | `0` |
| `saltation-oracle-1` | `running (healthy)` | `false` | `0` |
| `saltation-mongo-1` | `running (healthy)` | `false` | `0` |
| `saltation-web-1` | `running (healthy)` | `false` | `0` |
| `saltation-prometheus-1` | `running (healthy)` | `false` | `0` |
| `saltation-grafana-1` | `running (healthy)` | `false` | `0` |
| `saltation-loki-1` | `running (healthy)` | `false` | `0` |
| `saltation-alloy-1` | `running` | `false` | `0` |
| `saltation-stage8-fullrun` | `running` | `false` | `0` |
| `cloudflared-oci-vpc` | `running` | `false` | `0` |

- `/readyz` returned `{"status":"ok","oracle":"ok"}` on all `1,440` 1-minute samples.
- Host headroom at hour 24: **15 GiB** RAM available (`8.0 / 23 GiB` used, `0 B` swap), block volume `/home/ckh/block` at **40%** (`55G / 147G`, ~`+4 GB` over 24 hours; well within the N-9 70 GB budget).

## 4. Correctness & Operational Outcomes (Spec N-5 / F-36a)

- **HTTP 5xx responses**: **0**
- **Outbox relay errors (`bank_outbox_relay_errors_total`)**: **0**
- **Outbox backlog (`bank_outbox_undelivered`)**: remained near zero throughout (`12` at the 24-hour snapshot; never approached the F-36a 10,000 warning or 50,000 stop watermark)
- **Daily close (`bank_daily_close_mismatches`)**: **0 mismatches** across all completed rollovers

## 5. Follow-Up Findings (Spec N-4 Backfill Pacing)

The run missed the then-current N-4 target of completing the 365-day backfill within 24 hours because Argon2id authentication saturated the bank container's CPU. Raw pacing logs are kept only in ignored local `.evidence/`.

- `saltation-bank-1` saturated its `0.75 vCPU` limit. `saltation-oracle-1` had substantial headroom under its `1.50 vCPU` limit, and `saltation-mongo-1` and `saltation-stage8-fullrun` also had substantial headroom.
- Argon2id verification on `/v2.0/ext/auth/login` and `/atm/v1/session` dominated bank CPU usage. At this revision, `bank/src/shared/auth/mod.rs` hardcoded `19 MiB, t=2, p=1`. N-6 calls for configurable parameters that can be sized for N-4.

### Planned Next Changes
1. **N-6 Argon2id configuration**: make Argon2id cost parameters configurable via environment/config and size them so login stays within N-4 while preserving Argon2id security properties.
2. **Compose CPU budget rebalancing (within N-7 4 vCPU cap)**: shift unused CPU quota from `oracle` to `bank` (and align the Argon2 concurrency gate accordingly) while keeping total phase CPU within the N-7 4 vCPU cap.
3. **Generator session reuse audit**: verify whether repeated per-operation logins within the 30-minute idle window (F-2) can be reduced to match realistic customer session reuse.
4. **External connector quota**: add explicit Docker CPU (`0.15`) and memory (`128m`) limits to the host's `/home/ckh/docker/cloudflared-oci-vpc/compose.yaml` so `scripts/ops/check_compose_budget.py runtime --external-connector cloudflared-oci-vpc` passes without warning.
