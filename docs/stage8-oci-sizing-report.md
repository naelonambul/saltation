# Stage 8 OCI Resource Sizing, OWASP Argon2id Analysis, and Stage 6 Bottleneck Report

> **OTN compliance notice (`REVIEW.md`):** This public document records host/container resource budgets, standalone pure-Rust Argon2id CPU microbenchmarks, high-level pass/fail conformance outcomes, and generator/MongoDB bottleneck diagnostics only. Measured Oracle Database throughput, latency, wait-time, and execution-plan statistics are excluded and kept in ignored local `.evidence/` only.

## 1. Investigation Scope & Host Specifications

Following the 24-hour continuous stability run (`docs/stage8-24h-stability-report.md`), this investigation evaluated OCI ARM64 container resource sizing (Spec N-7), Argon2id cost parameters (Spec N-6, made configurable in PR #44), and Stage 6 backfill throughput bottlenecks (Spec N-4) on the primary development host.

| Property | Verified Value |
| --- | --- |
| **Host Shape** | OCI `VM.Standard.A1.Flex` — 4 physical Ampere Altra ARM64 cores (non-burstable), 24 GB RAM, 4 Gbps network |
| **Block Volume (`/home/ckh/block`)** | 150 GB Balanced (`10 VPU/GB`: 9,000 IOPS / 72 MB/s; online-scalable to `20 VPU/GB` = 11,250 IOPS if needed) |
| **Base Revision** | `7f6c522` (`main`, after PR #44 configurable Argon2id & auth histograms) |
| **N-7 Budget Gate** | `python3 scripts/ops/check_compose_budget.py runtime --external-connector cloudflared-oci-vpc --effective` **PASSED** after adding explicit `cpus: "0.15"` / `memory: 128m` to `/home/ckh/docker/cloudflared-oci-vpc/compose.yaml` |

## 2. Standalone Argon2id ARM64 Microbenchmark & OWASP Policy (Spec N-6)

Standalone pure-Rust `argon2 0.5.3` (`--release`, no database involvement) verification latency was measured on the OCI Ampere Altra host across OWASP Password Storage Cheat Sheet minimum configurations and exploratory non-OWASP profiles.

### 2.1 OWASP Password Storage Cheat Sheet Minimum Configurations

| Profile (`m_kib / t / p`) | OWASP Status | Measured `ms/verify` (1 vCPU) | Max Verifies/s per vCPU | Theoretical Max Mixed Ops/s at `38.7%` Auth Share (`bank` 1.30–1.50 vCPU) |
| --- | --- | ---: | ---: | --- |
| `46080 / 1 / 1` (~45–46 MiB) | OWASP min option 1 | `57.30 ms` | `17.5 /s` | `~58–68 ops/s` (~39–45h for 365d) |
| **`19456 / 2 / 1` (19 MiB)** | **OWASP min option 2 (current repo default — unchanged)** | **`32.39 ms`** | **`30.9 /s`** | **`~104–120 ops/s` (~22–25h for 365d)** |
| `12288 / 3 / 1` (12 MiB) | OWASP min option 3 | `29.37 ms` | `34.1 /s` | `~115–132 ops/s` (~20–23h for 365d) |
| `9216 / 4 / 1` (9 MiB) | OWASP min option 4 (reference only) | — | — | Equivalent security tier (`m*t = 36,864`) |
| `7168 / 5 / 1` (7 MiB) | OWASP min option 5 (reference only) | — | — | Equivalent security tier (`m*t = 35,840`, lowest RAM footprint) |

### 2.2 Exploratory Non-OWASP Profiles (Rejected)

| Profile (`m_kib / t / p`) | Status | Measured `ms/verify` (1 vCPU) | Note |
| --- | --- | ---: | --- |
| `8192 / 2 / 1` | **Rejected** | `12.92 ms` | Below OWASP Password Storage Cheat Sheet minimum |
| `4096 / 2 / 1` | **Rejected** | `6.15 ms` | Below OWASP minimum; used only to unblock secondary bottleneck profiling (Section 4) |
| `4096 / 1 / 1` | **Rejected** | `3.11 ms` | Below OWASP minimum |

**Policy decision:** Non-OWASP profiles are rejected. No new Argon2id default is adopted in this change; the repository default remains `19456 / 2 / 1` (OWASP minimum).

### 2.3 Structural Tension Between N-4, N-6, and N-7

Under the current Stage 6 persona workload (`generator/saltation_gen/personas/profiles.py`), 20,000 customers generate ~9.51M annual requests (`475.5` req/customer/yr), of which **`38.7%`** (`184` req/customer/yr = 155 password logins + 29 ATM PIN verifications) invoke Argon2id:

- Reaching N-4's `200 ops/s` sustained target at `38.7%` auth share requires **`77.4` Argon2id verifications/sec**.
- Under OWASP minimum configurations (`19456/2/1` at `30.9/s` per vCPU or `46080/1/1` at `17.5/s` per vCPU), `bank` within a 4 vCPU shared host budget (N-7) cannot physically sustain `77.4` verifications/sec without either:
  1. Reducing the normal-traffic auth share via realistic session token reuse (e.g., `38.7%` → `~10–15%`, which brings `200 ops/s` down to `20–30` verifications/sec), **or**
  2. Revising N-4's sustained ops/s target to align with OWASP Argon2id on 4 vCPU.

## 3. Candidate A Static Container Allocation (Spec N-7)

A static resource allocation (**Candidate A**) was tested within the N-7 budget (`<= 4.00 vCPU`, `<= 14 GB` RAM, including the external connector and active tool container):

| Container | Candidate A CPU | Candidate A RAM | Rationale |
| --- | ---: | ---: | --- |
| `oracle` | `1.05 vCPU` | `6 GiB` | Preserves Oracle 19c `PGA_AGGREGATE_LIMIT >= 2 GB` + `SGA 3.5 GB` floor; shifts unused CPU from initial `1.50 vCPU` |
| `bank` | `1.30 vCPU` | `1 GiB` | Expands CPU headroom from initial `0.75 vCPU` for Argon2id + API handling; peak RSS stayed `< 25 MiB` |
| `mongo` | `0.45 vCPU` | `3 GiB` | WiredTiger cache `1.5 GiB`; adequate for normal transfer outbox relay, saturated only during unbatched Stage 6 journaling |
| `generator` / `loadtest` (mutually exclusive tool) | `0.65 vCPU` | `768 MiB` | Drives N-1/N-2/N-3 loadtests or Stage 6 backfill |
| `web` | `0.05 vCPU` | `128 MiB` | Static SPA / nginx |
| `prometheus` + `grafana` + `loki` + `alloy` | `0.35 vCPU` | `2 GiB` | Observability stack |
| `cloudflared-oci-vpc` | `0.15 vCPU` | `128 MiB` | External edge connector |
| **Total (active phase)** | **`4.00 vCPU`** | **`13.96 GB`** | **Within N-7 (`<= 4 vCPU`, `<= 14 GB`)** |

### Exploratory Loadtest Outcomes on Candidate A

Exploratory stepped-load (N-3: 25/50/100/200 TPS, 60s/step) and hot-account (N-2: 219 senders → 1 receiver, 50 TPS, 60s) probes were executed against Candidate A:

- **N-1 / N-3 exploratory transfer latency & error rate**: **PASS** (all steps through 200 TPS stayed well within N-1/N-3 latency and `>= 99.9%` success criteria; `outbox_undelivered = 0`, `daily_close_mismatches = 0`, zero container restarts/OOMs).
- **N-2 exploratory hot-account probe**: **PASS** (`senders=219 >= 200`, `success_rate=100%`, well within the `<= 500 ms` p95 limit).
- Raw exploratory reports and server-side timings remain in ignored local `.evidence/` and OCI `/tmp/` only.

## 4. Stage 6 Backfill Secondary Bottleneck Diagnosis

When Argon2id CPU pressure was temporarily removed during an exploratory 3-day Stage 6 backfill slice (`20,000` customers, `amplification=20`, `workers=8`), `bank` remained mostly idle (`~20%` average CPU of `130%` quota, `0` cgroup throttling), yet backfill throughput plateaued far below N-4 because **`generator` and `mongo` saturated**:

- `saltation-sizing-3d` (`generator`): peaked at `65.2% / 65%` CPU limit (`nr_throttled` climbing continuously).
- `saltation-mongo-1` (`mongo`): peaked at `45.5% / 45%` CPU limit with heavy cgroup throttling.

Code inspection of `generator/saltation_gen/drivers/stage6.py`, `backfill_clock.py`, `stage6_execute.py`, `truth/stage6.py`, and `truth/recovery.py` confirmed the root cause:

1. **Per-wave clock & backpressure churn (average wave size ~3.3 operations):**
   - `BackfillClock.gate()` runs on every wave (rather than once per 60s bucket), performing 2 Mongo `WriteConcern(w=1, j=True)` progress writes plus `GET /internal/v1/outbox`.
   - `BackfillClock.move_clock()` performs `GET /internal/v1/clock`, 2 Mongo `j=True` progress writes, `POST /internal/v1/clock/set`, and a second `GET /internal/v1/clock` on each timestamp advance.
   - `clock.save(wave_date, wave_offset)` performs an additional Mongo `j=True` write after every wave.
2. **Per-operation serialized Mongo round-trips under a single global lock:**
   - `Stage6Journal._lock` (`asyncio.Lock`) serializes all 8 workers across every journal read/write.
   - Each individual business operation executes redundant `get_operation()` reads, `_require_owner()` double-reads (`leases` + `runs`), and 4 separate synchronous `WriteConcern(w=1, j=True)` writes (`prepare` → `ensure_labels` + readback → `transition(prepared→unconfirmed)` → `transition(unconfirmed→succeeded)`).
   - **Total overhead:** ~18 Mongo round-trips and ~5.5 synchronous `j=True` journal fsyncs per single business operation, serialized through one Python lock. Reallocating container CPU alone cannot overcome this serialization ceiling.

## 5. Recommended Follow-Up Changes

No runtime code or `compose.yml` defaults are modified by this documentation change. Future implementation packets should address:

1. **Stage 6 backfill journal & wave optimization (`generator/`):** move `BackfillClock.gate()` and `wave_offset` checkpoints to bucket granularity (preserving crash-recovery and F-34 pre-send label guarantees), batch per-wave Mongo operations (`bulk_write` / `insert_many`), and remove redundant `get_operation()` / `_require_owner()` / `GET /clock` round-trips.
2. **N-4 vs N-6 alignment:** decide whether to add realistic normal-customer session reuse in `generator/` (keeping OWASP Argon2id + `>= 200 ops/s`) or adjust N-4's sustained ops/s wording to match OWASP Argon2id on 4 vCPU.
3. **Static Compose & storage guardrails:** apply validated Candidate A CPU/RAM limits to `infra/compose/compose.yml`, add Prometheus `--storage.tsdb.retention.size=2GB`, and cap `saltation_data01.dbf` at `MAXSIZE 32G` in `infra/oracle/setup/01_saltation.sql` before running formal `loadtest --qualify all` and the 365-day backfill + 24-hour live qualification.
