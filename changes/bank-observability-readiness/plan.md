# Add session_id to app.log, /readyz readiness probe, and process RSS metrics

## Summary

Address the remaining N-12 observability and health-probe gaps identified in repository review:

1. **`session_id` in `app.log` (spec N-12):** `bank/src/shared/telemetry/mod.rs` omits `session_id` from per-request JSON log lines even though `AuthCustomer` already populates `IdentitySlot.session_id` and `auth::login` creates a new session ID.
2. **Liveness vs readiness separation:** `/healthz` currently returns a constant `"ok"` without checking Oracle reachability. Keep `/healthz` as fast liveness and add `/readyz` to verify Oracle connectivity (`SELECT 1 FROM DUAL`) without coupling transaction availability to transient MongoDB outbox-relay outages.
3. **Process memory visibility & early OOM alerting:** Expose `bank_process_resident_memory_bytes` (`VmRSS`) and `bank_process_anon_memory_bytes` (`RssAnon`) on `/metrics` (zero new dependencies, zero `unsafe`, parsed from `/proc/self/status` on Linux) and add a `SaltationBankMemoryHigh` (`> 750 MiB`) Prometheus alert rule.

## Files and components that change

| Paths | Work |
| --- | --- |
| `bank/src/shared/outcome/mod.rs` | Reuse an existing `IdentitySlot` extension in `Recorder::attach()` (`get_or_insert_default`) instead of overwriting it, so outer middleware (`request_log`) can read `session_id`. |
| `bank/src/shared/telemetry/mod.rs` | Insert `IdentitySlot` in `request_log` and emit `session_id` on every `app.log` request line; add `sample_process_memory()` and `/proc/self/status` `VmRSS`/`RssAnon` parser with unit tests. |
| `bank/src/features/auth/mod.rs` | Record the newly issued `session_id` into `IdentitySlot` on successful login so login lines in `app.log` include `session_id`. |
| `bank/src/app.rs` | Keep `/healthz` liveness `"ok"`, add `/readyz` Oracle readiness endpoint (`200` JSON when Oracle responds to `SELECT 1 FROM DUAL`, `503` otherwise; bypassed in `operation_routes`), and call `telemetry::sample_process_memory()` in `metrics_router` before `handle.render()`. |
| `infra/observability/prometheus/alerts.yml` | Add `SaltationBankMemoryHigh` (`bank_process_resident_memory_bytes > 786432000`, `for: 2m`, `severity: warning`). |
| `scripts/ops/test_retention.py` | Assert `SaltationBankMemoryHigh` expression in `alerts.yml`. |

## Order of work

1. **Wire `session_id` into `app.log`.** Update `Recorder::attach()` in `bank/src/shared/outcome/mod.rs` to reuse an existing `IdentitySlot`, insert/read `IdentitySlot` in `telemetry::request_log` (`bank/src/shared/telemetry/mod.rs`) to emit `session_id`, and populate `IdentitySlot.session_id` on successful login in `bank/src/features/auth/mod.rs`. Proof: `cargo test --manifest-path bank/Cargo.toml --locked --lib`.
2. **Add `/readyz` readiness probe and process RSS metrics.** Add `sample_process_memory()` and `/proc/self/status` parsing + unit tests in `bank/src/shared/telemetry/mod.rs`, mount `/readyz` and call `sample_process_memory()` in `bank/src/app.rs`. Proof: `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings` and `cargo test --manifest-path bank/Cargo.toml --locked --lib`.
3. **Add `SaltationBankMemoryHigh` alert and config test.** Update `infra/observability/prometheus/alerts.yml` and `scripts/ops/test_retention.py`. Proof: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_retention.py -v` and `python3 scripts/repo.py status --change bank-observability-readiness`.

## Tests and proof

- Rust fmt/clippy/unit tests: `cargo fmt --manifest-path bank/Cargo.toml --check`, `cargo clippy --manifest-path bank/Cargo.toml --locked --all-targets -- -D warnings`, `cargo test --manifest-path bank/Cargo.toml --locked --lib` (including `/proc/self/status` parser and `session_id` propagation tests).
- Python config tests: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_retention.py -v`.
- Control plane & full CI (`rust`, `python`, `oracle`, `control-plane`, `status`, `identity`, `summary`).

## Risks and mitigations

- **Platform portability of `/proc/self/status`:** `sample_process_memory()` returns `None` cleanly when `/proc/self/status` is absent (e.g. macOS local dev) without failing `/metrics`.
- **No coupling of `/readyz` to MongoDB:** `/readyz` checks Oracle only so transient MongoDB outbox-relay outages do not mark the transactional API unavailable (spec F-36).

## Rollback or recovery

Revert the scoped files; `/healthz` and existing metrics remain unchanged.
