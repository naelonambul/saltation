"""N-1, N-2, and N-3 transfer load-test scenarios and evidence report writers."""

from __future__ import annotations

import asyncio
import json
import random
from collections.abc import Callable
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path

from saltation_loadtest.client import Actor, LoadClient
from saltation_loadtest.runner import RunResult, run_open_loop
from saltation_loadtest.thresholds import (
    N3_STEP_SECONDS,
    N3_STEPS,
    CheckResult,
    ScenarioVerdict,
    StepOutcome,
    evaluate_n1,
    evaluate_n2,
    evaluate_n3,
)

SEED_BATCH = 100


async def prepare_actors(
    client: LoadClient,
    *,
    count: int,
    run_tag: int,
    opening_balance: int = 100_000_000,
    login_concurrency: int = 8,
) -> list[Actor]:
    """Seeds isolated loadtest customers/accounts and pre-authenticates all actors."""
    raw = []
    # Use a dedicated 80M-98M numeric range (account 9988..9989, phone 019..) to avoid collisions.
    base_num = 80_000_000 + (run_tag % 190) * 100_000
    for i in range(count):
        n = f"{base_num + i:08d}"
        raw.append(
            {
                "customer_no": f"L{n}",
                "name": f"부하{i:04d}",
                "phone": f"019{n}",
                "address": "서울",
                "customer_type": "INDIVIDUAL",
                "password": f"LtPass!{n}",
                "atm_pin": "1234",
                "account_no": f"998{n}",
                "opening_balance": opening_balance,
            }
        )

    fins: dict[str, str] = {}
    for start in range(0, len(raw), SEED_BATCH):
        batch = raw[start : start + SEED_BATCH]
        await client.seed_customers(
            [
                {k: c[k] for k in ("customer_no", "name", "phone", "address", "customer_type", "password")}
                for c in batch
            ]
        )
        accounts = await client.seed_accounts(
            [{k: c[k] for k in ("customer_no", "account_no", "atm_pin", "opening_balance")} for c in batch]
        )
        for acc in accounts:
            fins[acc["account_no"]] = acc["fintech_use_num"]

    sem = asyncio.Semaphore(login_concurrency)

    async def _login_one(idx: int, c: dict) -> Actor:
        ip = f"198.51.100.{(idx % 250) + 1}"
        region = "KR-11"
        device = f"lt-dev-{c['customer_no']}"
        async with sem:
            token = await client.login(c["phone"], c["password"], ip=ip, region=region, device=device)
        return Actor(
            customer_no=c["customer_no"],
            name=c["name"],
            phone=c["phone"],
            password=c["password"],
            account_no=c["account_no"],
            fintech_use_num=fins[c["account_no"]],
            token=token,
            ip=ip,
            region=region,
            device=device,
        )

    return list(await asyncio.gather(*(_login_one(i, c) for i, c in enumerate(raw))))


async def run_n1(
    client: LoadClient,
    actors: list[Actor],
    *,
    tps: int = 50,
    duration_seconds: float = 600.0,
    seed: int = 42,
) -> tuple[RunResult, ScenarioVerdict]:
    rng = random.Random(seed)
    n = len(actors)

    def pick(_seq: int) -> tuple[Actor, Actor]:
        i = rng.randrange(n)
        j = rng.randrange(n - 1)
        if j >= i:
            j += 1
        return actors[i], actors[j]

    res = await run_open_loop(client, target_tps=tps, duration_seconds=duration_seconds, pair_picker=pick)
    srv = res.server
    verdict = evaluate_n1(
        p50_ms=srv.transfer_p50_ms if srv else None,
        p95_ms=srv.transfer_p95_ms if srv else None,
        p99_ms=srv.transfer_p99_ms if srv else None,
        lock_wait_p95_ms=srv.lock_wait_p95_ms if srv else None,
        success_rate=res.non_business_success_rate,
    )
    complete = (
        tps == 50
        and duration_seconds == 600
        and res.workload_complete
        and res.server_complete
        and srv is not None
        and srv.lock_wait_count >= sum(a.succeeded for a in res.attempts) > 0
        and srv.lock_wait_p95_ms is not None
        and all(p is not None for p in (srv.transfer_p50_ms, srv.transfer_p95_ms, srv.transfer_p99_ms))
    )
    return res, require_evidence(verdict, complete)


async def run_n2(
    client: LoadClient,
    actors: list[Actor],
    *,
    tps: int = 50,
    duration_seconds: float = 300.0,
) -> tuple[RunResult, ScenarioVerdict]:
    receiver = actors[0]
    senders = actors[1:]

    def pick(seq: int) -> tuple[Actor, Actor]:
        return senders[seq % len(senders)], receiver

    res = await run_open_loop(client, target_tps=tps, duration_seconds=duration_seconds, pair_picker=pick)
    srv = res.server
    verdict = evaluate_n2(
        p95_ms=srv.transfer_p95_ms if srv else None,
        success_rate=res.non_business_success_rate,
        sender_count=len({a.sender for a in res.attempts}),
    )
    complete = (
        tps == 50
        and duration_seconds == 300
        and res.workload_complete
        and res.server_complete
        and srv is not None
        and srv.transfer_p95_ms is not None
        and {a.receiver for a in res.attempts} == {receiver.account_no}
    )
    return res, require_evidence(verdict, complete)


async def run_n3(
    client: LoadClient,
    actors: list[Actor],
    *,
    steps: tuple[int, ...] = N3_STEPS,
    step_duration_seconds: float = N3_STEP_SECONDS,
    seed: int = 99,
    capture_resource: Callable[[], dict] | None = None,
    classify_resource: Callable[[dict, dict], str] | None = None,
) -> tuple[list[RunResult], ScenarioVerdict]:
    rng = random.Random(seed)
    n = len(actors)

    def pick(_seq: int) -> tuple[Actor, Actor]:
        i = rng.randrange(n)
        j = rng.randrange(n - 1)
        if j >= i:
            j += 1
        return actors[i], actors[j]

    results: list[RunResult] = []
    outcomes: list[StepOutcome] = []
    for tps in steps:
        before = capture_resource() if capture_resource else None
        res = await run_open_loop(
            client, target_tps=tps, duration_seconds=step_duration_seconds, pair_picker=pick
        )
        results.append(res)
        srv = res.server
        p95 = srv.transfer_p95_ms if srv else None
        limiting = srv.limiting_resource if srv else "unknown"
        pool_full = sum(occupancy == capacity for _, occupancy, capacity in res.pool_samples) >= 2
        if srv and srv.pool_saturated and pool_full and limiting == "row_lock_wait":
            limiting = "ambiguous"
        if limiting == "db_pool" and not pool_full:
            limiting = "ambiguous"
        if before is not None and capture_resource and classify_resource:
            res.resource_before = before
            res.resource_after = capture_resource()
            host_limit = classify_resource(before, res.resource_after)
            if limiting == "unknown":
                limiting = host_limit
            elif host_limit not in ("unknown", limiting):
                limiting = "ambiguous"
        outcomes.append(
            StepOutcome(
                target_tps=tps,
                achieved_tps=res.achieved_tps,
                p95_ms=p95,
                success_rate=res.non_business_success_rate,
                limiting_resource=limiting,
                duration_seconds=res.duration_seconds,
                workload_complete=res.workload_complete,
                server_complete=res.server_complete,
                successes=sum(a.succeeded for a in res.attempts),
            )
        )

    return results, evaluate_n3(outcomes)


def require_evidence(verdict: ScenarioVerdict, complete: bool) -> ScenarioVerdict:
    if not complete:
        verdict.checks.append(
            CheckResult("Complete prescribed workload and server evidence", False, "incomplete")
        )
        verdict.passed = False
        verdict.evidence_complete = False
    return verdict


def write_evidence_report(
    report_dir: Path,
    scenario: str,
    payload: dict[str, object],
    verdict: ScenarioVerdict,
    *,
    exploratory: bool = True,
    run_id: str | None = None,
) -> tuple[Path, Path]:
    """Writes measured evidence strictly into git-ignored .evidence/loadtest/."""
    report_dir.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    stem = f"{run_id or ts}-{scenario.lower()}"
    outcome = "EXPLORATORY"
    if not exploratory:
        outcome = "INCOMPLETE"
        if verdict.evidence_complete:
            outcome = "PASS" if verdict.passed else "FAIL"
    json_path = report_dir / f"{stem}.json"
    md_path = report_dir / f"{stem}.md"

    doc = {
        "timestamp_utc": ts,
        "scenario": scenario,
        "outcome": outcome,
        "passed": verdict.passed if not exploratory else None,
        "checks": [asdict(c) for c in verdict.checks],
        "data": payload,
    }
    json_path.write_text(json.dumps(doc, indent=2, default=str) + "\n", encoding="utf-8")

    lines = [
        f"# Saltation Load Test Evidence: {scenario}",
        "",
        f"- **Timestamp (UTC)**: `{ts}`",
        f"- **Verdict**: `{outcome}`",
        "",
        "## Checks",
        "",
        "| Check | Status | Detail |",
        "| --- | --- | --- |",
    ]
    for c in verdict.checks:
        status = "observed" if exploratory else ("met" if c.passed else "unmet")
        lines.append(f"| {c.name} | {status} | `{c.detail}` |")
    lines.append("")
    lines.append("> OTN notice: Do not commit this file or copy measured Oracle timings into public text.")
    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return json_path, md_path
