# Align Stage 8 outbox alerts and N-9 retention script with spec F-36a and N-9

## Summary

Address the Stage 8 observability and retention gaps identified in repository review:

1. **Outbox alert thresholds (spec F-36a):** `infra/observability/prometheus/alerts.yml` currently alerts only on `bank_outbox_undelivered > 50000` (the backfill-stop watermark) and omits the F-36a warning threshold (`10,000` undelivered events) and oldest-age threshold (`bank_outbox_oldest_age_seconds > 30`).
2. **N-9 retention script precision & CLI contract:** `scripts/ops/retention.py` lacks the documented `--check-only` flag, uses a fixed `548`-day subtraction instead of calendar 18 months (`RETENTION_MONTHS = 18` is unused), and mixes binary GiB (`70 * 1024**3`) with decimal SI `parse_docker_size` (`GB = 1000**3`), shifting the effective 80% threshold from `56 GB` to `~60.13 GB`.
3. **Test assertions:** `scripts/ops/test_retention.py` only checks alert-name substrings and does not validate PromQL expressions, thresholds, calendar month clamping, decimal GB consistency, or CLI flags.

## Files and components that change

| Paths | Work |
| --- | --- |
| `infra/observability/prometheus/alerts.yml` | Set `SaltationOutboxBacklogHigh` to `bank_outbox_undelivered > 10000` (`warning`), add `SaltationOutboxBacklogCritical` for `bank_outbox_undelivered >= 50000` (`critical`), and add `SaltationOutboxOldestAgeHigh` for `bank_outbox_oldest_age_seconds > 30` (`warning`). |
| `scripts/ops/retention.py` | Switch `BUDGET_BYTES` and GB reporting to decimal SI (`70 * 1000**3` = `70 GB`, 80% = `56 GB`) matching `parse_docker_size` and spec N-9; replace `RETENTION_DAYS = 548` with calendar 18-month subtraction (`compute_bank_logs_cutoff(..., months=18)` with end-of-month clamping); add `--check-only` mutually exclusive with `--apply`. |
| `scripts/ops/test_retention.py` | Assert exact alert rules and PromQL thresholds in `alerts.yml`; test calendar 18-month subtraction (regular date, month-end non-leap, month-end leap), decimal 70 GB / 56 GB 80% boundary, and `--check-only` / `--apply` parser behavior. |

## Order of work

1. **Update Prometheus outbox alert rules.** Edit `infra/observability/prometheus/alerts.yml` to cover `bank_outbox_undelivered > 10000`, `bank_outbox_undelivered >= 50000`, and `bank_outbox_oldest_age_seconds > 30`. Proof: `python3 -m unittest scripts/ops/test_retention.py -v`.
2. **Fix N-9 retention math, units, and CLI flags.** Edit `scripts/ops/retention.py` to use decimal SI `70 * 1000**3` bytes (consistent with `parse_docker_size`), calendar 18-month cutoff with month-end clamping, and mutually exclusive `--check-only` / `--apply` flags. Proof: `python3 -m unittest scripts/ops/test_retention.py -v`.
3. **Strengthen unit and config tests.** Update `scripts/ops/test_retention.py` to parse and verify each alert's `expr` and threshold, calendar 18-month edge cases, decimal 80% threshold (`56 GB`), and CLI argument parsing. Proof: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_retention.py -v` and `python3 scripts/repo.py status --change stage8-alerts-retention-hardening`.

## Tests and proof

- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_retention.py -v`
- `python3 scripts/repo.py status --change stage8-alerts-retention-hardening`
- CI `control-plane`, `python`, `status`, `identity`, `summary` checks.

## Risks and mitigations

- **Decimal vs binary GB shift:** Aligning `BUDGET_BYTES` to `70_000_000_000` bytes makes the 80% alert stricter (`56.0 GB` instead of `60.13 GB`), which matches `spec.md` N-9 (`70 GB`) and `docker system df -v` SI output without false passes.
- **Prometheus container/disk exporter:** Host/volume disk metrics in Prometheus remain outside this packet (handled separately with N-12 container resource metrics if adopted) so `compose.yml` resource budgets are unaffected here.

## Rollback or recovery

Revert the three scoped files (`infra/observability/prometheus/alerts.yml`, `scripts/ops/retention.py`, `scripts/ops/test_retention.py`).
