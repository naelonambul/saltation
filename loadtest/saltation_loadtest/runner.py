"""Open-loop constant-rate transfer workload runner."""

from __future__ import annotations

import asyncio
import math
import time
from collections import Counter
from collections.abc import Callable
from dataclasses import dataclass, field

import httpx

from saltation_loadtest.client import Actor, LoadClient, TransferAttempt
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
    expected: int = 0
    missed: int = 0
    late: int = 0
    dispatch: list[tuple[int, float]] = field(default_factory=list)
    attempts: list[TransferAttempt] = field(default_factory=list)
    business_rejections: dict[str, int] = field(default_factory=dict)
    drain_seconds: float = 0.0
    server_completion_tps: float | None = None
    metrics_error: str | None = None
    metrics_before: str | None = None
    metrics_after: str | None = None
    resource_before: dict | None = None
    resource_after: dict | None = None
    pool_samples: list[tuple[float, float, float]] = field(default_factory=list)

    @property
    def workload_complete(self) -> bool:
        return self.expected > 0 and self.total_sent == self.expected and not (self.late or self.missed)

    @property
    def server_complete(self) -> bool:
        if self.server is None:
            return False
        received = self.total_sent - self.status_counts.get(0, 0)
        return (
            0 < received <= self.server.transfer_count <= self.total_sent
            and sum(self.server.status_counts.values()) == self.server.transfer_count
            and all(
                self.server.status_counts.get(status, 0) >= count
                for status, count in self.status_counts.items()
                if status != 0
            )
        )


def percentile(sorted_values: list[float], q: float) -> float | None:
    if not sorted_values:
        return None
    idx = min(len(sorted_values) - 1, max(0, math.ceil(q * len(sorted_values)) - 1))
    return sorted_values[idx]


def compute_success_rate(attempts: list[TransferAttempt]) -> float:
    eligible = [attempt for attempt in attempts if attempt.business_rejection is None]
    return sum(attempt.succeeded for attempt in eligible) / len(eligible) if eligible else 0.0


async def run_open_loop(
    client: LoadClient,
    *,
    target_tps: int,
    duration_seconds: float,
    pair_picker: Callable[[int], tuple[Actor, Actor]],
    amount: int = 100,
    max_in_flight: int = 256,
    scrape_metrics: bool = True,
) -> RunResult:
    """Dispatch into a fixed window. Record missed arrivals rather than queue a catch-up burst."""
    if target_tps <= 0 or not math.isfinite(duration_seconds) or duration_seconds <= 0:
        raise ValueError("rate and duration must be positive and finite")
    if max_in_flight < 1:
        raise ValueError("max_in_flight must be positive")

    metrics_error = None
    before = None
    metrics_before = metrics_after = None
    if scrape_metrics:
        try:
            metrics_before = await client.scrape_metrics()
            before = MetricsSnapshot.parse(metrics_before)
        except (httpx.HTTPError, ValueError) as exc:
            metrics_error = f"initial metrics unavailable ({type(exc).__name__})"

    expected = math.ceil(target_tps * duration_seconds)
    interval = 1.0 / target_tps
    # Allow modest event-loop jitter without allowing requests to be batched across the window.
    tolerance = min(0.010, 2 * interval)
    dispatch: list[tuple[int, float]] = []
    attempts: list[TransferAttempt] = []
    in_flight = missed = late = 0
    started = time.perf_counter()
    end = started + duration_seconds

    async def one(seq: int, due: float, sender: Actor, receiver: Actor) -> None:
        nonlocal in_flight, late, missed
        try:
            now = time.perf_counter()
            if now >= end:
                missed += 1
                return
            dispatch.append((seq, now - started))
            late += now - due > tolerance
            attempts.append(await client.transfer(sender, receiver, amount))
        finally:
            in_flight -= 1

    pool_samples = []

    async def sample_pool() -> None:
        while time.perf_counter() < end:
            try:
                async with asyncio.timeout(1.0):
                    snap = MetricsSnapshot.parse(await client.scrape_metrics())
                capacity = snap.gauge_max("bank_db_pool_capacity")
                occupied = snap.gauge_max("bank_db_pool_in_use")
                if math.isfinite(capacity) and capacity > 0 and 0 <= occupied <= capacity:
                    pool_samples.append((time.perf_counter() - started, occupied, capacity))
            except (TimeoutError, httpx.HTTPError):
                pass
            await asyncio.sleep(1.0)

    async with asyncio.TaskGroup() as tasks:
        sampler = tasks.create_task(sample_pool()) if before is not None and duration_seconds > 2 else None
        for seq in range(expected):
            due = started + seq * interval
            delay = due - time.perf_counter()
            if delay > 0:
                await asyncio.sleep(delay)
            now = time.perf_counter()
            if now >= end or now - due > tolerance or in_flight >= max_in_flight:
                missed += 1
                continue
            sender, receiver = pair_picker(seq)
            in_flight += 1
            tasks.create_task(one(seq, due, sender, receiver))
        if (remaining := end - time.perf_counter()) > 0:
            await asyncio.sleep(remaining)
        if sampler is not None:
            sampler.cancel()

    drain = max(0.0, time.perf_counter() - end)
    server = None
    if before is not None:
        try:
            metrics_after = await client.scrape_metrics()
            server = summarize_window(before, MetricsSnapshot.parse(metrics_after))
        except (httpx.HTTPError, ValueError) as exc:
            metrics_error = f"final metrics unavailable ({type(exc).__name__})"
    latencies = sorted(a.elapsed_ms for a in attempts)
    return RunResult(
        target_tps=target_tps,
        duration_seconds=duration_seconds,
        total_sent=len(dispatch),
        achieved_tps=len(dispatch) / duration_seconds,
        status_counts=dict(Counter(a.status for a in attempts)),
        non_business_success_rate=compute_success_rate(attempts),
        client_p50_ms=percentile(latencies, 0.50),
        client_p95_ms=percentile(latencies, 0.95),
        client_p99_ms=percentile(latencies, 0.99),
        server=server,
        expected=expected,
        missed=missed,
        late=late,
        dispatch=dispatch,
        attempts=attempts,
        business_rejections=dict(Counter(a.business_rejection for a in attempts if a.business_rejection)),
        drain_seconds=drain,
        server_completion_tps=server.transfer_count / (duration_seconds + drain) if server else None,
        metrics_error=metrics_error,
        metrics_before=metrics_before,
        metrics_after=metrics_after,
        pool_samples=pool_samples,
    )
