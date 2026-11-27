#!/usr/bin/env python3

from __future__ import annotations

import argparse
import calendar
import json
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

BUDGET_BYTES = 70 * 1000**3
ALERT_RATIO = 0.80
RETENTION_MONTHS = 18
MAX_APP_LOG_FILES = 7

SALTATION_VOLUMES = (
    "saltation_oradata",
    "saltation_mongodata",
    "saltation_applogs",
    "saltation_lokidata",
    "saltation_promdata",
    "saltation_grafanadata",
    "saltation_alloydata",
)

_SIZE_RE = re.compile(r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([kKmMgGtT]?B)\s*$")
_UNIT_MULT = {
    "B": 1,
    "KB": 1000,
    "MB": 1000**2,
    "GB": 1000**3,
    "TB": 1000**4,
}


def compute_bank_logs_cutoff(now: datetime | None = None, months: int = RETENTION_MONTHS) -> str:
    ref = now or datetime.now(UTC)
    if ref.tzinfo is None:
        ref = ref.replace(tzinfo=UTC)
    ref_utc = ref.astimezone(UTC)
    total_months = ref_utc.year * 12 + (ref_utc.month - 1) - months
    year, zero_month = divmod(total_months, 12)
    month = zero_month + 1
    day = min(ref_utc.day, calendar.monthrange(year, month)[1])
    cutoff = ref_utc.replace(year=year, month=month, day=day)
    return cutoff.strftime("%Y-%m-%dT%H:%M:%S.000Z")


def parse_docker_size(size_str: str) -> int:
    m = _SIZE_RE.match(size_str)
    if not m:
        raise ValueError(f"Unrecognized docker size string: {size_str!r}")
    val = float(m.group(1))
    unit = m.group(2).upper()
    return int(round(val * _UNIT_MULT[unit]))


@dataclass
class DiskBudgetReport:
    volumes: dict[str, int]
    total_bytes: int
    budget_bytes: int
    usage_ratio: float
    alert: bool


def evaluate_disk_budget(
    volumes: dict[str, int],
    *,
    budget_bytes: int = BUDGET_BYTES,
    alert_ratio: float = ALERT_RATIO,
) -> DiskBudgetReport:
    total = sum(volumes.values())
    ratio = total / budget_bytes if budget_bytes > 0 else 1.0
    return DiskBudgetReport(
        volumes=volumes,
        total_bytes=total,
        budget_bytes=budget_bytes,
        usage_ratio=ratio,
        alert=ratio >= alert_ratio,
    )


def evaluate_app_log_files(filenames: list[str], max_files: int = MAX_APP_LOG_FILES) -> list[str]:
    pattern = re.compile(r"^app\.\d{4}-\d{2}-\d{2}\.log$")
    daily = sorted(f for f in filenames if pattern.match(f))
    if len(daily) <= max_files:
        return []
    return daily[: len(daily) - max_files]


def measure_saltation_volumes(project_prefix: str = "saltation_") -> dict[str, int]:
    proc = subprocess.run(
        ["docker", "system", "df", "-v", "--format", "{{json .Volumes}}"],
        check=True,
        capture_output=True,
        text=True,
    )
    raw = proc.stdout.strip()
    if not raw or raw == "null":
        return {}
    entries = json.loads(raw)
    result: dict[str, int] = {}
    for item in entries:
        name = item.get("Name", "")
        if name.startswith(project_prefix) and not name.startswith("saltation-it_"):
            result[name] = parse_docker_size(item.get("Size", "0B"))
    return result


def check_mongo_expired(
    cutoff_iso: str, *, apply: bool, container: str, mongo_env: dict[str, str]
) -> int:
    js_action = (
        f'db.events.deleteMany({{recorded_at: {{$lt: "{cutoff_iso}"}}}}).deletedCount'
        if apply
        else f'db.events.countDocuments({{recorded_at: {{$lt: "{cutoff_iso}"}}}})'
    )
    shell_cmd = (
        'mongosh --quiet -u bank -p "$MONGO_BANK_PASSWORD" '
        '--authenticationDatabase bank_logs bank_logs --eval "$1"'
    )
    proc = subprocess.run(
        ["docker", "exec", "-e", "MONGO_BANK_PASSWORD", container, "sh", "-c", shell_cmd, "_", f"print({js_action})"],
        check=True,
        capture_output=True,
        text=True,
        env=mongo_env,
    )
    return int(proc.stdout.strip().splitlines()[-1])

def check_applogs_in_container(container: str, *, apply: bool) -> list[str]:
    proc = subprocess.run(
        ["docker", "exec", container, "ls", "-1", "/var/log/saltation"],
        check=True,
        capture_output=True,
        text=True,
    )
    excess = evaluate_app_log_files(proc.stdout.splitlines())
    if apply and excess:
        subprocess.run(
            ["docker", "exec", container, "rm", "-f", *(f"/var/log/saltation/{f}" for f in excess)],
            check=True,
        )
    return excess


def load_env_file(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    if not path.exists():
        return env
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        env[k.strip()] = v.strip()
    return env


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Saltation N-9 retention and 70 GB disk budget check")
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="Delete expired bank_logs and excess app.log files")
    mode.add_argument(
        "--check-only",
        action="store_true",
        help="Report budget and retention status without deleting data (default)",
    )
    p.add_argument("--env-file", default="infra/compose/.env", help="Path to compose .env file")
    p.add_argument("--mongo-container", default="saltation-mongo-1")
    p.add_argument("--bank-container", default="saltation-bank-1")
    return p


def main() -> None:
    args = build_parser().parse_args()
    env = load_env_file(Path(args.env_file))
    mongo_env = dict(os.environ)
    if "MONGO_BANK_PASSWORD" not in mongo_env and "MONGO_BANK_PASSWORD" in env:
        mongo_env["MONGO_BANK_PASSWORD"] = env["MONGO_BANK_PASSWORD"]

    cutoff = compute_bank_logs_cutoff()
    volumes = measure_saltation_volumes()
    budget = evaluate_disk_budget(volumes)

    expired_events: int | None = None
    if mongo_env.get("MONGO_BANK_PASSWORD"):
        expired_events = check_mongo_expired(
            cutoff, apply=args.apply, container=args.mongo_container, mongo_env=mongo_env
        )

    excess_logs = check_applogs_in_container(args.bank_container, apply=args.apply)

    report = {
        "mode": "apply" if args.apply else "check-only",
        "bank_logs_cutoff_utc": cutoff,
        "bank_logs_expired_or_deleted": expired_events,
        "app_log_excess_files": excess_logs,
        "disk_budget": asdict(budget),
        "disk_usage_gb": round(budget.total_bytes / (1000**3), 2),
        "disk_budget_gb": round(budget.budget_bytes / (1000**3), 2),
        "disk_usage_pct": round(budget.usage_ratio * 100, 2),
    }
    print(json.dumps(report, indent=2))
    if budget.alert:
        print(
            f"ALERT: Saltation volume usage ({report['disk_usage_pct']}%) exceeded 80% of 70 GB budget!",
            file=sys.stderr,
        )
        raise SystemExit(2)


if __name__ == "__main__":
    main()
