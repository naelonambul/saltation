import json
import re
import unittest
from datetime import UTC, datetime
from pathlib import Path

from scripts.ops.retention import (
    BUDGET_BYTES,
    build_parser,
    compute_bank_logs_cutoff,
    evaluate_app_log_files,
    evaluate_disk_budget,
    parse_docker_size,
)

ROOT = Path(__file__).resolve().parents[2]


def _parse_alert_expressions(text: str) -> dict[str, str]:
    alerts: dict[str, str] = {}
    current: str | None = None
    for line in text.splitlines():
        m_alert = re.match(r"^\s*-\s*alert:\s*(\S+)\s*$", line)
        if m_alert:
            current = m_alert.group(1)
            continue
        m_expr = re.match(r"^\s*expr:\s*(.+?)\s*$", line)
        if m_expr and current is not None:
            alerts[current] = m_expr.group(1)
    return alerts


class RetentionTests(unittest.TestCase):
    def test_cutoff_iso_utc_calendar_months(self) -> None:
        now = datetime(2026, 11, 21, 12, 0, 0, tzinfo=UTC)
        self.assertEqual(compute_bank_logs_cutoff(now), "2025-05-21T12:00:00.000Z")
        non_leap = datetime(2026, 8, 31, 0, 0, 0, tzinfo=UTC)
        self.assertEqual(compute_bank_logs_cutoff(non_leap), "2025-02-28T00:00:00.000Z")
        leap = datetime(2025, 8, 31, 0, 0, 0, tzinfo=UTC)
        self.assertEqual(compute_bank_logs_cutoff(leap), "2024-02-29T00:00:00.000Z")

    def test_parse_docker_size(self) -> None:
        self.assertEqual(parse_docker_size("0B"), 0)
        self.assertEqual(parse_docker_size("500kB"), 500_000)
        self.assertEqual(parse_docker_size("2.5GB"), 2_500_000_000)

    def test_disk_budget_decimal_gb_and_80_percent_alert(self) -> None:
        self.assertEqual(BUDGET_BYTES, 70_000_000_000)
        ok = evaluate_disk_budget({"saltation_oradata": parse_docker_size("55.9GB")})
        self.assertFalse(ok.alert)
        warn = evaluate_disk_budget({"saltation_oradata": parse_docker_size("56GB")})
        self.assertTrue(warn.alert)

    def test_app_log_excess_files(self) -> None:
        files = [f"app.2026-11-{d:02d}.log" for d in range(1, 10)] + ["other.txt"]
        self.assertEqual(evaluate_app_log_files(files), ["app.2026-11-01.log", "app.2026-11-02.log"])
        self.assertEqual(evaluate_app_log_files(files[:7]), [])

    def test_cli_check_only_and_apply_flags(self) -> None:
        parser = build_parser()
        default_args = parser.parse_args([])
        self.assertFalse(default_args.apply)
        check_only_args = parser.parse_args(["--check-only"])
        self.assertFalse(check_only_args.apply)
        self.assertTrue(check_only_args.check_only)
        apply_args = parser.parse_args(["--apply"])
        self.assertTrue(apply_args.apply)
        with self.assertRaises(SystemExit):
            parser.parse_args(["--apply", "--check-only"])


class ObservabilityConfigTests(unittest.TestCase):
    def test_grafana_dashboard_has_required_n12_panels(self) -> None:
        dash = json.loads((ROOT / "infra/observability/grafana/dashboards/bank.json").read_text(encoding="utf-8"))
        titles = {p["title"] for p in dash["panels"]}
        for required in (
            "Transfer latency p50 / p95 / p99 (N-1)",
            "Outbox backlog (F-36a)",
            "Row-lock wait p95 (ledger.lock_accounts, N-1)",
            "Outbox oldest event age (F-36a)",
            "Daily-close mismatches (N-5)",
            "5xx error rate (%)",
            "app.log",
        ):
            self.assertIn(required, titles)

    def test_prometheus_and_loki_retention_and_alerts(self) -> None:
        prom = (ROOT / "infra/observability/prometheus/prometheus.yml").read_text(encoding="utf-8")
        alerts_text = (ROOT / "infra/observability/prometheus/alerts.yml").read_text(encoding="utf-8")
        loki = (ROOT / "infra/observability/loki/loki.yaml").read_text(encoding="utf-8")
        self.assertIn("alerts.yml", prom)
        self.assertIn("retention_period: 336h", loki)

        exprs = _parse_alert_expressions(alerts_text)
        self.assertEqual(exprs.get("SaltationOutboxBacklogHigh"), "bank_outbox_undelivered > 10000")
        self.assertEqual(exprs.get("SaltationOutboxBacklogCritical"), "bank_outbox_undelivered >= 50000")
        self.assertEqual(exprs.get("SaltationOutboxOldestAgeHigh"), "bank_outbox_oldest_age_seconds > 30")
        self.assertEqual(exprs.get("SaltationDailyCloseMismatch"), "bank_daily_close_mismatches > 0")
        self.assertIn("> 0.3", exprs.get("SaltationTransferLatencyHigh", ""))
        self.assertEqual(
            exprs.get("SaltationBankMemoryHigh"),
            "bank_process_resident_memory_bytes > 786432000",
        )


if __name__ == "__main__":
    unittest.main()
