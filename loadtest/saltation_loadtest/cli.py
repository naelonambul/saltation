"""CLI entrypoint for Saltation N-1, N-2, and N-3 transfer load tests."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from dataclasses import asdict
from pathlib import Path

from saltation_loadtest.client import LoadClient
from saltation_loadtest.scenarios import (
    prepare_actors,
    run_n1,
    run_n2,
    run_n3,
    write_evidence_report,
)


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
    p.add_argument("--senders", type=int, default=220, help="Total pre-seeded actors (default: 220)")
    p.add_argument("--smoke", action="store_true", help="Short 10s smoke durations instead of full spec runs")
    p.add_argument("--duration", type=float, default=None, help="Override scenario/step duration in seconds")
    p.add_argument("--tps", type=int, default=50, help="Target TPS for n1/n2 (default: 50)")
    p.add_argument("--steps", default="25,50,100,200", help="Comma-separated TPS steps for n3")
    p.add_argument("command", choices=["n1", "n2", "n3", "all"], help="Scenario to execute")
    return p


async def _run(args: argparse.Namespace) -> int:
    report_dir = Path(args.report_dir)
    client = LoadClient(args.bank_url, args.metrics_url)
    try:
        if not await client.healthz():
            print("bank /healthz is not 200", file=sys.stderr)
            return 2

        run_tag = int(time.time()) % 900
        actor_count = max(args.senders, 201 if args.command in ("n2", "all") else 20)
        actors = await prepare_actors(client, count=actor_count, run_tag=run_tag)

        overall_ok = True
        summary: list[dict[str, object]] = []

        if args.command in ("n1", "all"):
            dur = args.duration if args.duration is not None else (10.0 if args.smoke else 600.0)
            res, verdict = await run_n1(client, actors, tps=args.tps, duration_seconds=dur)
            jp, mp = write_evidence_report(report_dir, "n1", asdict(res), verdict)
            overall_ok = overall_ok and verdict.passed
            summary.append({"scenario": "N-1", "passed": verdict.passed, "report": str(mp), "json": str(jp)})

        if args.command in ("n2", "all"):
            dur = args.duration if args.duration is not None else (10.0 if args.smoke else 300.0)
            res, verdict = await run_n2(client, actors, tps=args.tps, duration_seconds=dur)
            jp, mp = write_evidence_report(report_dir, "n2", asdict(res), verdict)
            overall_ok = overall_ok and verdict.passed
            summary.append({"scenario": "N-2", "passed": verdict.passed, "report": str(mp), "json": str(jp)})

        if args.command in ("n3", "all"):
            dur = args.duration if args.duration is not None else (10.0 if args.smoke else 180.0)
            steps = tuple(int(x.strip()) for x in args.steps.split(",") if x.strip())
            results, verdict = await run_n3(client, actors, steps=steps, step_duration_seconds=dur)
            jp, mp = write_evidence_report(report_dir, "n3", {"steps": [asdict(r) for r in results]}, verdict)
            overall_ok = overall_ok and verdict.passed
            summary.append({"scenario": "N-3", "passed": verdict.passed, "report": str(mp), "json": str(jp)})

        print(json.dumps({"passed": overall_ok, "results": summary}, indent=2))
        return 0 if overall_ok else 1
    finally:
        await client.close()


def main() -> None:
    args = build_parser().parse_args()
    raise SystemExit(asyncio.run(_run(args)))
