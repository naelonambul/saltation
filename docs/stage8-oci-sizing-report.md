# Stage 8 OCI Resource Sizing, OWASP Argon2id Analysis, and Stage 6 Bottleneck Report

> **OTN compliance notice (`REVIEW.md`).** This report records host and container resource budgets, standalone pure-Rust Argon2id CPU microbenchmarks, conformance outcomes, and bottleneck diagnostics. Oracle-backed application throughput, latency, wait-time, execution-plan, and benchmark-derived wall-time and extrapolation measurements are excluded and kept only in ignored local `.evidence/`.

## 1. Investigation Scope & Host Specifications

Following the 24-hour continuous stability run (`docs/stage8-24h-stability-report.md`), this investigation evaluated OCI ARM64 container resource sizing (Spec N-7), Argon2id cost parameters (Spec N-6, made configurable in PR #44), and Stage 6 backfill throughput bottlenecks (Spec N-4) on the primary development host.

| Property | Verified Value |
| --- | --- |
| **Host Shape** | OCI `VM.Standard.A1.Flex` — 4 physical Ampere Altra ARM64 cores (non-burstable), 24 GB RAM, 4 Gbps network |
| **Block Volume (`/home/ckh/block`)** | 150 GB Balanced (`10 VPU/GB`: 9,000 IOPS / 72 MB/s; online-scalable to `20 VPU/GB` = 11,250 IOPS if needed) |
| **Base Revision** | `7f6c522` (initial sizing & PR #44) → `e3a960a` (`main`, post-PR #46 Stage 6 backfill batching & PR #47–#50 CI hardening) |
| **N-7 Budget Gate** | `python3 scripts/ops/check_compose_budget.py runtime --external-connector cloudflared-oci-vpc --effective` **PASSED** after adding explicit `cpus: "0.15"` / `memory: 128m` to `/home/ckh/docker/cloudflared-oci-vpc/compose.yaml` |

## 2. Standalone Argon2id ARM64 Microbenchmark & OWASP Policy (Spec N-6)

Standalone pure-Rust `argon2 0.5.3` (`--release`, no database involvement) verification latency was measured on the OCI Ampere Altra host across OWASP Password Storage Cheat Sheet minimum configurations and exploratory non-OWASP profiles.

The theoretical mixed operation rates and 365-day estimates below use only the standalone Rust timings, the workload's authentication share, and the CPU budget.

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

- Reaching the then-current N-4 target of `200 ops/s` sustained at `38.7%` auth share requires **`77.4` Argon2id verifications/sec**.
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
- Raw exploratory reports and server-side timings are kept only in ignored local `.evidence/`.

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

## 5. Post-PR #46 Stage 6 Backfill Validation & OWASP `19456/2/1` Results

PR #46 (`ab742da`, validated on `main` at `e3a960a`) implemented the Stage 6 backfill batching optimizations diagnosed in Section 4 (`BackfillClock` bucket-granularity gating and direct `POST /clock/set` response validation, cached card networks / home context, 256-operation lock-free normal waves with batched `prepare_batch` / `ensure_labels_batch` / `mark_unconfirmed_wave` / `complete_wave`, `$group` totals aggregation, and outbox relay 35 ms partial-batch linger with `{recorded_at: 1}` indexing).

Two complete 3-day Stage 6 backfill slices (`20,000` customers, `amplification=20`, `workers=8`, `81,930` operations + all `3,400` annual `A..G` + `lookalike` scenarios compressed into 3 days) were then executed on OCI under Candidate A (`4.00 vCPU / 13.96 GB`):

1. **Exploratory `4096/2/1` (`concurrency=3`) A/B run:** isolates the generator/MongoDB speedup from PR #46.
2. **Production-compliant OWASP minimum `19456/2/1` (`concurrency=3`) run:** measures real end-to-end performance at the repository's default OWASP Argon2id cost.

### 5.1 Three-Way Comparison (Candidate A, 3 Days / 20,000 Customers / `workers=8`)

| Metric | Pre-PR #46 (`4096/2/1`, `7f6c522`) | Post-PR #46 (`4096/2/1`, `e3a960a`) | Post-PR #46 (**OWASP `19456/2/1`**, `e3a960a`) |
| --- | --- | --- | --- |
| **Control calls** | Per-wave gating with redundant clock reads | Bucket-level gating with direct clock-set response validation reduced control calls | Same batched control path |
| **Primary bottleneck** | Serialized journaling saturated `generator` and `mongo` | Load was more balanced after batching | **`bank` Argon2id CPU** |
| **`bank` resident memory (RSS)** | `~24 MiB` | `24.0 MiB` | **`24.7 MiB`** (`< 2.5%` of `1 GiB` limit) |
| **Conformance (`rejected` / `unresolved` / `daily_close_mismatches` / `outbox_undelivered`)** | Incomplete run | `0 / 0 / 0 / 0` (`81,930` succeeded) | **`0 / 0 / 0 / 0`** (`81,930` succeeded, `369` daily closes) |

### 5.2 OWASP `19456/2/1` Resource Observations

After PR #46 reduced generator and MongoDB serialization, Argon2id hashing during seeding and verification during backfill logins and PIN checks made bank CPU the primary bottleneck under the OWASP profile. Raw timings and rates are kept only in ignored local `.evidence/`.

| Container | Observed memory | Candidate A memory limit |
| --- | ---: | ---: |
| `bank` (RSS) | `24.7 MiB` | `1 GiB` |
| `mongo` | `341.8 MiB` | `3 GiB` |
| `generator` | `137.5 MiB` | `768 MiB` |
| `oracle` | `4.29 GiB` | `6 GiB` |

## 6. Post-Hardening OCI Validation (`main` @ `de0d055`, PR #52–#60)

Following the PR #52–#60 hardening sequence (`Argon2Pool` + `ClockCache` in PR #52, Candidate A Compose/storage guardrails in PR #53, Stage 6 365-day resilience & traffic quality in PR #54/#56, rollover batching in PR #55, `bank` P1 hardening & monetary outcome unification in PR #57/#59/#60, and F-27a daily midnight maintenance window + F-32 normal mobile session reuse + N-4/N-6 alignment in PR #58), two end-to-end validation runs were executed on the primary OCI Ampere A1 host (`4.00 vCPU / 13.96 GB` Candidate A budget, OWASP `19456/2/1`, `SALTATION_ARGON2_CONCURRENCY=3`, `SALTATION_MAINTENANCE_MINUTES=30`):

### 6.1 Transfer Load Test (`saltation-loadtest all`, Exploratory Run `20261125T124225Z-f5d30baecaf7`)

Per the repository's OTN benchmark-disclosure policy (`REVIEW.md`), raw latency and wait measurements stay in the ignored `.evidence/loadtest/` directory; only pass/fail outcomes and stability observations are recorded here:

- **N-1 (50 TPS × 10 min random transfers):** met all latency, lock-wait, and success-rate thresholds (`100.000%` non-business success rate, `0` deadlock retries, `0` daily-close mismatches).
- **N-2 (50 TPS × 5 min hot account, 219 senders):** met all latency and success-rate thresholds (`100.000%` non-business success rate).
- **N-3 (25 / 50 / 100 / 200 TPS × 3 min stepped load):** met all thresholds across every step (`100.00%` non-business success rate, `0` deadlock retries, `0` daily-close mismatches, `limiting_resource: none_saturated` through the 200 TPS step).
- **Container & co-tenant stability (N-7 / N-8):** all 8 Saltation containers and `cloudflared-oci-vpc` remained `healthy` with `0` restarts and `0` OOM events.

### 6.2 Stage 6 3-Day OWASP `19456/2/1` Backfill (`oci-3d-de0d055`)

A clean-volume 3-day Stage 6 backfill (`20,000` customers, `amplification=20`, `workers=8`, `first=2025-01-02`, `days=3`) completed with zero errors:

- **Workload changes.** PR #56 spread operations across each hour to reduce bursts and added Scenario G declined-card operations. PR #58 introduced session reuse for normal mobile visits to reduce repeated logins, with greater impact on the normal-traffic mix of a full-year run. Raw run timings are kept only in ignored local `.evidence/`.
- **Operations & scenario conformance:** `75,798` succeeded + `200` declined (`200/200` Scenario G `card.declined` generated), `0` unresolved, `0` prepared, `0` daily-close mismatches (`bank_daily_close_mismatches=0`), and `0` undelivered outbox events (`bank_outbox_undelivered=0`). Every annual scenario quota (`A: 200, B: 200, C: 100, D: 100, E: 300, F: 300, G: 200, lookalike: 2,000`) reached 100%.
- **Observed container memory & health at completion:**
  - `saltation-bank-1`: `79.0 MiB / 1 GiB` (`healthy`, `0` restarts)
  - `saltation-mongo-1`: `346.3 MiB / 3 GiB` (`healthy`, `0` restarts)
  - `saltation-grafana-1`: `105.2 MiB / 256 MiB` (`healthy`, `0` restarts)
  - `saltation-oracle-1`: `4.27 GiB / 6 GiB` (`healthy`, `0` restarts)
  - `cloudflared-oci-vpc`: `29.5 MiB / 128 MiB` (`healthy`, `0` restarts)

## 7. Remaining Follow-Up Milestones

1. ~~**Stage 6 backfill journal & wave optimization (`generator/`):**~~ **Completed in PR #46 (`ab742da`)** and validated in Section 5.
2. ~~**N-4 vs N-6 alignment & normal mobile session reuse:**~~ **Completed in PR #58 (`0c02df4`)** — F-32 `normal_visit` (75%/25% mix), F-27a daily midnight maintenance window, and N-4/N-6 alignment (`>= 55 ops/s`, `<= 48h` under OWASP `19456/2/1` on 4 vCPU).
3. ~~**Static Compose & storage guardrails:**~~ **Completed in PR #53 (`1f335b2`)** and PR #59 (`6995d3b`) — Candidate A CPU/RAM limits in `infra/compose/compose.yml`, Prometheus `2GB` TSDB cap, and `MAXSIZE 30G` in both `01_saltation.sql` and `oracle-init.sql`.
4. **Formal qualification & Stage 7 Web UI:** execute formal `saltation-loadtest all --qualify --revision <sha>`, the full 365-day backfill + 24-hour live run on OCI, and complete Stage 7 Web UI (`web/apps/admin` F-39 and remaining `web/apps/mobile` F-38 screens).
