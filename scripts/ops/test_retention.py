import json
import unittest
from datetime import UTC, datetime
from pathlib import Path

from scripts.ops.retention import (
    BUDGET_BYTES,
    compute_bank_logs_cutoff,
    evaluate_app_log_files,
    evaluate_disk_budget,
    parse_docker_size,
)

ROOT = Path(__file__).resolve().parents[2]


class RetentionTests(unittest.TestCase):
    def test_cutoff_iso_utc(self) -> None:
        now = datetime(2026, 11, 21, 12, 0, 0, tzinfo=UTC)
        cutoff = compute_bank_logs_cutoff(now, days=548)
        self.assertEqual(cutoff, "2025-05-22T12:00:00.000Z")

    def test_parse_docker_size(self) -> None:
        self.assertEqual(parse_docker_size("0B"), 0)
        self.assertEqual(parse_docker_size("500kB"), 500_000)
        self.assertEqual(parse_docker_size("2.5GB"), 2_500_000_000)

    def test_disk_budget_alert_at_80_percent(self) -> None:
        ok = evaluate_disk_budget({"saltation_oradata": int(BUDGET_BYTES * 0.79)})
        self.assertFalse(ok.alert)
        warn = evaluate_disk_budget({"saltation_oradata": int(BUDGET_BYTES * 0.80)})
        self.assertTrue(warn.alert)

    def test_app_log_excess_files(self) -> None:
        files = [f"app.2026-11-{d:02d}.log" for d in range(1, 10)] + ["other.txt"]
        self.assertEqual(evaluate_app_log_files(files), ["app.2026-11-01.log", "app.2026-11-02.log"])
        self.assertEqual(evaluate_app_log_files(files[:7]), [])


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
        alerts = (ROOT / "infra/observability/prometheus/alerts.yml").read_text(encoding="utf-8")
        loki = (ROOT / "infra/observability/loki/loki.yaml").read_text(encoding="utf-8")
        self.assertIn("alerts.yml", prom)
        self.assertIn("SaltationOutboxBacklogHigh", alerts)
        self.assertIn("SaltationDailyCloseMismatch", alerts)
        self.assertIn("retention_period: 336h", loki)


if __name__ == "__main__":
    unittest.main()
