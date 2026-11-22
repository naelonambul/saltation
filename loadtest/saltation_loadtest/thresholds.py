"""Spec N-1, N-2, N-3, and N-5 pass/fail evaluators."""

from __future__ import annotations

import math
from dataclasses import dataclass, field


@dataclass
class CheckResult:
    name: str
    passed: bool
    detail: str


@dataclass
class ScenarioVerdict:
    scenario: str
    passed: bool
    checks: list[CheckResult] = field(default_factory=list)
    evidence_complete: bool = True


def evaluate_n1(
    *,
    p50_ms: float | None,
    p95_ms: float | None,
    p99_ms: float | None,
    lock_wait_p95_ms: float | None,
    success_rate: float,
) -> ScenarioVerdict:
    checks = [
        CheckResult(
            "p50 <= 50ms",
            p50_ms is not None and p50_ms <= 50.0,
            f"p50={p50_ms:.2f}ms" if p50_ms is not None else "missing p50",
        ),
        CheckResult(
            "p95 <= 300ms",
            p95_ms is not None and p95_ms <= 300.0,
            f"p95={p95_ms:.2f}ms" if p95_ms is not None else "missing p95",
        ),
        CheckResult(
            "p99 <= 1000ms",
            p99_ms is not None and p99_ms <= 1000.0,
            f"p99={p99_ms:.2f}ms" if p99_ms is not None else "missing p99",
        ),
        CheckResult(
            "row_lock_wait_p95 <= 50ms",
            lock_wait_p95_ms is not None and lock_wait_p95_ms <= 50.0,
            f"lock_p95={lock_wait_p95_ms:.2f}ms" if lock_wait_p95_ms is not None else "missing lock p95",
        ),
        CheckResult(
            "success_rate >= 99.9%",
            success_rate >= 0.999,
            f"success_rate={success_rate * 100:.3f}%",
        ),
    ]
    return ScenarioVerdict(scenario="N-1", passed=all(c.passed for c in checks), checks=checks)


def evaluate_n2(
    *,
    p95_ms: float | None,
    success_rate: float,
    sender_count: int,
) -> ScenarioVerdict:
    checks = [
        CheckResult(
            "senders >= 200",
            sender_count >= 200,
            f"senders={sender_count}",
        ),
        CheckResult(
            "p95 <= 500ms",
            p95_ms is not None and p95_ms <= 500.0,
            f"p95={p95_ms:.2f}ms" if p95_ms is not None else "missing p95",
        ),
        CheckResult(
            "success_rate >= 99.9%",
            success_rate >= 0.999,
            f"success_rate={success_rate * 100:.3f}%",
        ),
    ]
    return ScenarioVerdict(scenario="N-2", passed=all(c.passed for c in checks), checks=checks)


N3_STEPS = (25, 50, 100, 200)
N3_STEP_SECONDS = 180.0


@dataclass
class StepOutcome:
    target_tps: int
    achieved_tps: float
    p95_ms: float | None
    success_rate: float
    limiting_resource: str
    duration_seconds: float = 0.0
    workload_complete: bool = False
    server_complete: bool = False
    successes: int = 0


def evaluate_n3(steps: list[StepOutcome]) -> ScenarioVerdict:
    """Evaluate ordered, fully evidenced steps before interpreting saturation."""
    if tuple(s.target_tps for s in steps) != N3_STEPS or any(
        s.duration_seconds != N3_STEP_SECONDS
        or not s.workload_complete
        or s.achieved_tps != s.target_tps
        or not s.server_complete
        or s.p95_ms is None
        or not math.isfinite(s.p95_ms)
        or s.p95_ms < 0
        for s in steps
    ):
        return ScenarioVerdict(
            "N-3",
            False,
            [CheckResult("Complete prescribed workload and server evidence", False, "incomplete")],
            evidence_complete=False,
        )

    crossing = next((s for s in steps if s.p95_ms is not None and s.p95_ms > 300.0), None)
    resource_known = crossing is None or crossing.limiting_resource in {
        "row_lock_wait",
        "disk",
        "cpu",
        "db_pool",
    }
    passing_threshold = crossing is None or crossing.target_tps > 100
    detail = (
        "No crossing observed through the 200 TPS step"
        if crossing is None
        else f"first step > 300ms: {crossing.target_tps} TPS (limiting={crossing.limiting_resource})"
    )
    successful_load = all(
        step.successes > 0 and (step is crossing or step.success_rate >= 0.999) for step in steps
    )
    checks = [
        CheckResult("first_p95_over_300ms > 100 TPS", passing_threshold, detail),
        CheckResult("successful load before saturation", successful_load, "success counters by step"),
        CheckResult("limiting resource supported", resource_known, detail),
    ]
    return ScenarioVerdict(
        "N-3", passing_threshold and resource_known and successful_load, checks, resource_known
    )


def evaluate_n5(
    *,
    daily_close_mismatches: int,
    duplicate_movements: int,
    f21_violations: int,
) -> ScenarioVerdict:
    checks = [
        CheckResult("daily_close_mismatches == 0", daily_close_mismatches == 0, f"{daily_close_mismatches}"),
        CheckResult("duplicate_movements == 0", duplicate_movements == 0, f"{duplicate_movements}"),
        CheckResult("f21_violations == 0", f21_violations == 0, f"{f21_violations}"),
    ]
    return ScenarioVerdict(scenario="N-5", passed=all(c.passed for c in checks), checks=checks)
