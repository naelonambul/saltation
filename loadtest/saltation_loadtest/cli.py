"""CLI entrypoint for Saltation N-1, N-2, and N-3 transfer load tests."""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import platform
import re
import sys
import time
from dataclasses import asdict
from datetime import UTC, datetime, timedelta, timezone
from pathlib import Path
from uuid import uuid4

import httpx

from saltation_loadtest import verify_host, verify_oracle
from saltation_loadtest.client import LoadClient
from saltation_loadtest.scenarios import (
    prepare_actors,
    require_evidence,
    run_n1,
    run_n2,
    run_n3,
    write_evidence_report,
)
from saltation_loadtest.thresholds import N3_STEP_SECONDS, N3_STEPS, CheckResult, evaluate_n5


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="saltation-loadtest", description="Saltation transfer load tests")
    p.add_argument(
        "--bank-url",
        default=os.environ.get("SALTATION_BANK_URL", "http://bank:8081"),
        help="Bank generator listener URL (default: http://bank:8081)",
    )
    p.add_argument(
        "--metrics-url",
        default=os.environ.get("SALTATION_METRICS_URL", "http://bank:9100"),
        help="Bank metrics listener URL (default: http://bank:9100)",
    )
    p.add_argument(
        "--report-dir",
        default=os.environ.get("SALTATION_REPORT_DIR", "/evidence/loadtest"),
        help="Directory for JSON/MD evidence reports (must stay git-ignored)",
    )
    p.add_argument(
        "--revision",
        default=os.environ.get("SALTATION_VERIFY_REVISION"),
        help="Deployed Git revision, required for qualification",
    )
    p.add_argument(
        "--wait-close",
        type=float,
        default=0,
        help="Seconds to wait for normal daily close after all qualification workloads",
    )
    p.add_argument("--senders", type=int, default=220, help="Total pre-seeded actors (default: 220)")
    p.add_argument("--qualify", action="store_true", help="Require complete primary-host acceptance evidence")
    p.add_argument("--smoke", action="store_true", help="Short 10s smoke durations instead of full spec runs")
    p.add_argument("--duration", type=float, default=None, help="Override scenario/step duration in seconds")
    p.add_argument("--tps", type=int, default=50, help="Target TPS for n1/n2 (default: 50)")
    p.add_argument("--steps", default="25,50,100,200", help="Comma-separated TPS steps for n3")
    p.add_argument("command", choices=["n1", "n2", "n3", "all"], help="Scenario to execute")
    return p


KST = timezone(timedelta(hours=9))


def maintenance_window_conflict(
    clock_state: dict | None, duration_seconds: float, maintenance_minutes: int
) -> str | None:
    if not clock_state or maintenance_minutes <= 0:
        return None
    raw_time = clock_state.get("business_time")
    if not isinstance(raw_time, str):
        return None
    start = datetime.fromisoformat(raw_time).astimezone(KST)
    mode = str(clock_state.get("mode", "")).upper()

    def in_window(dt: datetime) -> bool:
        return dt.hour == 0 and dt.minute < maintenance_minutes

    if in_window(start):
        return (
            f"bank business_time {start.isoformat()} is inside the daily maintenance window "
            f"(00:00-00:{maintenance_minutes:02d} KST)"
        )
    if mode == "LIVE":
        end = start + timedelta(seconds=duration_seconds)
        if in_window(end) or end.date() > start.date():
            return (
                f"LIVE loadtest window {start.isoformat()}..{end.isoformat()} overlaps the daily "
                f"maintenance window (00:00-00:{maintenance_minutes:02d} KST)"
            )
    return None


def parse_steps(raw: str) -> tuple[int, ...]:
    return tuple(int(step) for step in raw.split(","))


def write_result(report_dir: Path, run_id: str, outcome: str, **evidence: object) -> int:
    result = {
        "run_id": run_id,
        "outcome": outcome,
        "passed": outcome == "PASS" if outcome != "EXPLORATORY" else None,
    }
    result.update(evidence)
    report_dir.mkdir(parents=True, exist_ok=True)
    path = report_dir / f"{run_id}-summary.json"
    path.write_text(json.dumps(result, indent=2, default=str) + "\n", encoding="utf-8")
    print(json.dumps({"run_id": run_id, "outcome": outcome, "summary": str(path)}))
    return {"PASS": 0, "FAIL": 1, "INCOMPLETE": 2, "EXPLORATORY": 0}[outcome]


def merge_oracle_proof(proof: dict, verdict, data: dict) -> bool:
    values = {key: proof[key] for key in ("daily_close_mismatches", "duplicate_movements", "f21_violations")}
    if any(type(value) is not int or value < 0 for value in values.values()):
        raise ValueError("invalid Oracle counters")
    result = evaluate_n5(**values)
    data["n5"] = proof
    verdict.checks.extend(result.checks)
    verdict.passed = verdict.passed and result.passed
    return result.passed


async def resolve_daily_close(verifier, max_wait: float) -> dict:
    deadline = time.monotonic() + max_wait
    while True:
        try:
            return await verifier.verify_daily_close()
        except verify_oracle.OracleProofIncomplete:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise
            await asyncio.sleep(min(30.0, remaining))


async def _run(args: argparse.Namespace) -> int:
    report_dir = Path(args.report_dir)
    run_id = f"{datetime.now(UTC):%Y%m%dT%H%M%SZ}-{uuid4().hex[:12]}"
    try:
        steps = parse_steps(args.steps)
        if args.tps <= 0 or not steps or any(step <= 0 for step in steps):
            raise ValueError("rates must be positive")
        if args.duration is not None and (args.duration <= 0 or not math.isfinite(args.duration)):
            raise ValueError("duration must be positive and finite")
        if args.qualify and (
            args.command != "all"
            or args.smoke
            or args.duration is not None
            or args.tps != 50
            or steps != N3_STEPS
            or args.senders < 201
        ):
            raise ValueError(
                "qualification requires all with the prescribed rates, durations and sender count"
            )
        if not math.isfinite(args.wait_close) or args.wait_close < 0:
            raise ValueError("wait-close must be finite and nonnegative")
        if args.qualify and not re.fullmatch(r"[0-9a-f]{40}", args.revision or ""):
            raise ValueError("qualification requires the deployed 40-character Git revision")
    except ValueError as exc:
        return write_result(report_dir, run_id, "INCOMPLETE", errors=[str(exc)])

    summary = []
    incomplete = []
    incomplete_errors = False
    failures = []
    host_result = None
    host = None
    pending = []
    started_at = datetime.now(UTC).isoformat()
    client = LoadClient(args.bank_url, args.metrics_url)
    try:
        if args.qualify:
            host = await verify_host.HostVerifier.start()
            bound = host.bind_targets(
                bank_url=args.bank_url,
                metrics_url=args.metrics_url,
                oracle_dsn=os.environ.get("SALTATION_DB_CONNECT", ""),
                oracle_user=verify_oracle.oracle_schema(),
                revision=args.revision,
            )
            if not bound or host.incomplete or host.violations:
                host_result = await host.finish()
                return write_result(
                    report_dir,
                    run_id,
                    "FAIL" if host_result["violations"] else "INCOMPLETE",
                    host=host_result,
                    revision=args.revision,
                )

        if not await client.healthz():
            raise RuntimeError("bank health check failed")
        maint_minutes = int(os.environ.get("SALTATION_MAINTENANCE_MINUTES", "30").strip() or "30")
        clock_state = None
        if maint_minutes > 0:
            clock_getter = getattr(client, "clock_state", None)
            if callable(clock_getter):
                clock_state = await clock_getter()
            elif hasattr(client, "_http"):
                try:
                    resp = await client._http.get("/internal/v1/clock")
                    if resp.status_code == 200:
                        clock_state = resp.json()
                except Exception:
                    clock_state = None
            planned_secs = sum(
                (args.duration if args.duration is not None else (10.0 if args.smoke else default_d))
                * (len(steps) if name == "n3" else 1)
                for name, default_d in (("n1", 600.0), ("n2", 300.0), ("n3", N3_STEP_SECONDS))
                if args.command in (name, "all")
            )
            conflict = maintenance_window_conflict(clock_state, planned_secs, maint_minutes)
            if conflict:
                return write_result(
                    report_dir,
                    run_id,
                    "INCOMPLETE",
                    errors=[conflict],
                    revision=args.revision,
                )
        run_tag = int(time.time()) % 900
        actor_count = max(args.senders, 201 if args.command in ("n2", "all") else 20)
        actors = await prepare_actors(client, count=actor_count, run_tag=run_tag)
        accounts = [a.account_no for a in actors]

        for name, default_duration in (("n1", 600.0), ("n2", 300.0), ("n3", N3_STEP_SECONDS)):
            if args.command not in (name, "all"):
                continue
            print(f"{run_id}: starting {name}", file=sys.stderr, flush=True)
            duration = (
                args.duration if args.duration is not None else (10.0 if args.smoke else default_duration)
            )
            oracle = None
            if args.qualify:
                oracle = await verify_oracle.OracleVerifier.start(accounts)
            if name == "n3":
                res, verdict = await run_n3(
                    client,
                    actors,
                    steps=steps,
                    step_duration_seconds=duration,
                    capture_resource=verify_host.capture_resource if host is not None else None,
                    classify_resource=verify_host.classify_resource if host is not None else None,
                )
                windows = res
            else:
                scenario = run_n1 if name == "n1" else run_n2
                window, verdict = await scenario(client, actors, tps=args.tps, duration_seconds=duration)
                windows = [window]
            data = {"windows": [asdict(window) for window in windows]}
            if oracle is not None:
                try:
                    proof = await oracle.verify([asdict(a) for window in windows for a in window.attempts])
                    if not merge_oracle_proof(proof, verdict, data):
                        failures.append(f"{name}: N-5")
                except verify_oracle.OracleProofIncomplete as exc:
                    data["n5"] = exc.diagnostics
                    if any(value > 0 for value in exc.diagnostics.get("observed_counters", {}).values()):
                        failures.append(f"{name}: N-5")
                    if args.wait_close and exc.diagnostics.get("pending_daily_close"):
                        pending.append((name, oracle, data, verdict))
                        continue
                    verdict = require_evidence(verdict, False)
                    verdict.checks.append(CheckResult("N-5 Oracle proof", False, str(exc)))
                except (ValueError, RuntimeError, KeyError) as exc:
                    verdict = require_evidence(verdict, False)
                    verdict.checks.append(CheckResult("N-5 Oracle proof", False, type(exc).__name__))
            if not verdict.evidence_complete:
                incomplete.append(f"{name}: incomplete workload, metric or verification evidence")
            elif not verdict.passed:
                failures.append(name)
            json_path, md_path = write_evidence_report(
                report_dir,
                name,
                data,
                verdict,
                exploratory=not args.qualify,
                run_id=run_id,
            )
            summary.append({"scenario": verdict.scenario, "json": str(json_path), "report": str(md_path)})
        if pending:
            print(f"{run_id}: waiting for normal daily close", file=sys.stderr, flush=True)
        wait_until = time.monotonic() + args.wait_close
        for name, oracle, data, verdict in pending:
            try:
                proof = await resolve_daily_close(oracle, max(0.0, wait_until - time.monotonic()))
                if not merge_oracle_proof(proof, verdict, data):
                    failures.append(f"{name}: N-5")
            except verify_oracle.OracleProofIncomplete as exc:
                data["n5"] = exc.diagnostics
                verdict = require_evidence(verdict, False)
                verdict.checks.append(CheckResult("N-5 Oracle proof", False, str(exc)))
            if not verdict.evidence_complete:
                incomplete.append(f"{name}: incomplete workload, metric or verification evidence")
            elif not verdict.passed:
                failures.append(name)
            json_path, md_path = write_evidence_report(
                report_dir,
                name,
                data,
                verdict,
                exploratory=False,
                run_id=run_id,
            )
            summary.append({"scenario": verdict.scenario, "json": str(json_path), "report": str(md_path)})
        if host is not None:
            host_result = await host.finish()
            incomplete.extend(host_result["incomplete"])
            failures.extend(host_result["violations"])
    except (ValueError, RuntimeError, OSError, httpx.HTTPError) as exc:
        incomplete_errors = True
        incomplete.append(f"run could not finish ({type(exc).__name__})")
    finally:
        if host is not None:
            await host.close()
        await client.close()

    if args.qualify:
        outcome = "FAIL" if failures else "PASS"
        if outcome != "FAIL" and (incomplete or host_result is None or len(summary) != 3):
            outcome = "INCOMPLETE"
    else:
        expected_scenarios = 3 if args.command == "all" else 1
        outcome = (
            "EXPLORATORY" if len(summary) == expected_scenarios and not incomplete_errors else "INCOMPLETE"
        )
    return write_result(
        report_dir,
        run_id,
        outcome,
        results=summary,
        errors=incomplete,
        failures=failures,
        host=host_result,
        revision=args.revision,
        driver_architecture=platform.machine(),
        command=args.command,
        started_at=started_at,
        finished_at=datetime.now(UTC).isoformat(),
    )


def main() -> None:
    args = build_parser().parse_args()
    raise SystemExit(asyncio.run(_run(args)))
