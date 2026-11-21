"""Open-loop constant-rate transfer workload runner."""

from __future__ import annotations

import asyncio
import math
import time
from collections import Counter
from collections.abc import Callable
from dataclasses import dataclass, field

from saltation_loadtest.client import Actor, LoadClient
from saltation_loadtest.metrics import MetricsSnapshot, ServerWindowMetrics, summarize_window


@dataclass
class RunResult:
    target_tps: int
    duration_seconds: float
    total_sent: int
    achieved_tps: float
    status_counts: dict[int, int]
    non_business_success_rate: float
    client_p50_ms: float | None
    client_p95_ms: float | None
    client_p99_ms: float | None
    server: ServerWindowMetrics | None = None
    extra: dict[str, object] = field(default_factory=dict)


def percentile(sorted_values: list[float], q: float) -> float | None:
    if not sorted_values:
        return None
    idx = min(len(sorted_values) - 1, max(0, math.ceil(q * len(sorted_values)) - 1))
    return sorted_values[idx]


def compute_success_rate(status_counts: dict[int, int]) -> float:
    """Spec N-1/N-2: business rejections (422) excluded from the failure denominator."""
    ok = status_counts.get(200, 0)
    eligible = sum(cnt for code, cnt in status_counts.items() if code != 422)
    if eligible <= 0:
        return 0.0
    return ok / eligible


async def run_open_loop(
    client: LoadClient,
    *,
    target_tps: int,
    duration_seconds: float,
    pair_picker: Callable[[int], tuple[Actor, Actor]],
    amount: int = 100,
    max_in_flight: int = 512,
    scrape_metrics: bool = True,
) -> RunResult:
    """Schedules requests at fixed inter-arrival intervals to avoid coordinated omission."""
    before_snap = MetricsSnapshot.parse(await client.scrape_metrics()) if scrape_metrics else None

    sem = asyncio.Semaphore(max_in_flight)
    latencies: list[float] = []
    counts: Counter[int] = Counter()
    tasks: set[asyncio.Task[None]] = set()

    total_expected = max(1, int(round(target_tps * duration_seconds)))
    interval = 1.0 / target_tps
    started = time.perf_counter()

    async def _one(seq: int) -> None:
        sender, receiver = pair_picker(seq)
        try:
            status, elapsed_ms = await client.transfer(sender, receiver, amount)
            counts[status] += 1
            latencies.append(elapsed_ms)
        finally:
            sem.release()

    for seq in range(total_expected):
        target_time = started + seq * interval
        delay = target_time - time.perf_counter()
        if delay > 0:
            await asyncio.sleep(delay)
        await sem.acquire()
        task = asyncio.create_task(_one(seq))
        tasks.add(task)
        task.add_done_callback(tasks.discard)

    if tasks:
        await asyncio.gather(*tasks)

    elapsed_total = max(1e-6, time.perf_counter() - started)
    after_snap = MetricsSnapshot.parse(await client.scrape_metrics()) if scrape_metrics else None
    server_metrics = (
        summarize_window(before_snap, after_snap)
        if before_snap is not None and after_snap is not None
        else None
    )

    latencies.sort()
    status_dict = dict(counts)
    return RunResult(
        target_tps=target_tps,
        duration_seconds=elapsed_total,
        total_sent=len(latencies),
        achieved_tps=len(latencies) / elapsed_total,
        status_counts=status_dict,
        non_business_success_rate=compute_success_rate(status_dict),
        client_p50_ms=percentile(latencies, 0.50),
        client_p95_ms=percentile(latencies, 0.95),
        client_p99_ms=percentile(latencies, 0.99),
        server=server_metrics,
    )
