# Complete Stage 8 Grafana dashboards, Prometheus alerts, and N-9 retention checks

## Summary

Complete the Stage 8 observability and storage-retention deliverables (`spec.md` N-9, N-12, Acceptance Criteria #11 and #13). Expand the provisioned Grafana `Saltation bank` dashboard with row-lock wait, outbox age, daily-close mismatch, and error-rate panels; add Prometheus alert rules (`alerts.yml`) covering outbox backlog, daily-close mismatches, and the 80% disk-budget threshold; and add an idempotent retention and volume-budget check tool (`scripts/ops/retention.py`) with unit tests covering MongoDB `bank_logs` 18-month pruning (respecting the ISO-8601 string `recorded_at` constraint), `app.log` 7-file rotation verification, and Saltation volume usage against the 70 GB budget (alerting at 80% = 56 GB).

## Files and components that change

| Path | Change |
| --- | --- |
| `infra/observability/grafana/dashboards/bank.json` | Add N-12 panels: row-lock wait p95 (`ledger.lock_accounts`), outbox oldest age (`bank_outbox_oldest_age_seconds`), daily-close mismatches (`bank_daily_close_mismatches`), deadlock retries & outbox relay errors, and 5xx error rate (%). |
| `infra/observability/prometheus/alerts.yml`, `infra/observability/prometheus/prometheus.yml` | Add Prometheus alerting rules for outbox backlog (`> 50000`), daily-close mismatches (`> 0`), outbox relay errors, and disk budget (`> 80%` of 70 GB), and wire `rule_files` in `prometheus.yml`. |
| `infra/compose/compose.yml` | Mount `infra/observability/prometheus/alerts.yml` read-only into the `prometheus` container. |
| `scripts/ops/retention.py`, `scripts/ops/test_retention.py` | Implement N-9 retention helper (18-month `bank_logs.events` cutoff calculation & pruning, `app.log` 7-file check, Saltation Docker volume usage vs 70 GB / 80% threshold alert) plus unit tests and observability config validation tests (validating `bank.json`, `prometheus.yml`, `alerts.yml`, `loki.yaml`). |
| `checks.json` | Register `ops-observability` check under the `core` group routed to `infra/observability/**` and `scripts/ops/**`. |

## Order of work

1. **Expand Grafana dashboard and Prometheus alert rules** (`infra/observability/grafana/dashboards/bank.json`, `infra/observability/prometheus/alerts.yml`, `infra/observability/prometheus/prometheus.yml`, `infra/compose/compose.yml`).
   - Keep existing 6 panels in `bank.json` intact and add panels for row-lock wait p95, outbox oldest age, daily-close mismatches, deadlock/relay errors, and 5xx error ratio.
   - Add `alerts.yml` with PromQL alert rules and mount it in `compose.yml`.
   - Proof: `docker compose -f infra/compose/compose.yml --env-file infra/compose/.env.example --profile local config -q` and `promtool check rules` inside `prom/prometheus:v3.5.0`.

2. **Implement N-9 retention & 80% disk-budget check with tests** (`scripts/ops/retention.py`, `scripts/ops/test_retention.py`, `checks.json`).
   - Compute 18-month UTC ISO-8601 cutoff for `bank_logs.events.recorded_at` without introducing BSON Date fields.
   - Measure Saltation data volumes (`saltation_oradata`, `saltation_mongodata`, `saltation_applogs`, `saltation_lokidata`, `saltation_promdata`, `saltation_grafanadata`, `saltation_alloydata`) and report WARN/ALERT exit status when total exceeds 80% of 70 GB (56 GB).
   - Add stdlib `unittest` suite (`scripts/ops/test_retention.py`) verifying cutoff math, budget alert thresholds, `app.log` 7-file check, and structural validity of `bank.json`, `prometheus.yml`, `alerts.yml`, and `loki.yaml`.
   - Register `ops-observability` in `checks.json` under `core` group.
   - Proof: `python3 -m unittest scripts/ops/test_retention.py -v` and `python3 scripts/repo.py verify --change stage8-observability-retention --group core`.

3. **Live verification on OCI**
   - Reload `prometheus` and `grafana` on OCI, verify `/api/v1/rules` loads `alerts.yml` cleanly, verify Grafana `/api/health` and provisioned `Saltation bank` dashboard panels, and run `scripts/ops/retention.py --check-only` against the live OCI volumes to confirm Saltation disk usage is well within the 70 GB budget.

## Risks and mitigations

- **Invalid PromQL or Grafana JSON breaking provisioning on startup.** Validate `alerts.yml` with `promtool check rules` and validate `bank.json` structure in `test_retention.py` before reloading containers.
- **Accidental deletion of active `bank_logs` data.** Default `scripts/ops/retention.py` to dry-run / `--check-only` unless `--apply` is explicitly passed.

## Rollback or recovery

Revert the added dashboard panels, `alerts.yml`, `scripts/ops/`, and `checks.json` entry; restart `prometheus` and `grafana`. No database data is modified.

## Open questions

None.
