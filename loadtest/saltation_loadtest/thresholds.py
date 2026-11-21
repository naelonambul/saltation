"""Spec N-1, N-2, N-3, and N-5 pass/fail evaluators."""

from __future__ import annotations

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


@dataclass
class StepOutcome:
    target_tps: int
    achieved_tps: float
    p95_ms: float | None
    success_rate: float
    limiting_resource: str


def evaluate_n3(steps: list[StepOutcome]) -> ScenarioVerdict:
    """First step whose p95 exceeds 300 ms must be > 100 TPS (spec N-3)."""
    first_exceed: StepOutcome | None = None
    for step in sorted(steps, key=lambda s: s.target_tps):
        if step.p95_ms is None or step.p95_ms > 300.0:
            first_exceed = step
            break

    passed_threshold = first_exceed is None or first_exceed.target_tps > 100
    detail = (
        "no step exceeded 300ms"
        if first_exceed is None
        else f"first step > 300ms: {first_exceed.target_tps} TPS (limiting={first_exceed.limiting_resource})"
    )
    checks = [
        CheckResult("first_p95_over_300ms > 100 TPS", passed_threshold, detail),
        CheckResult(
            "all steps <= 100 TPS >= 99.9% success",
            all(s.success_rate >= 0.999 for s in steps if s.target_tps <= 100),
            ", ".join(f"{s.target_tps}tps:{s.success_rate * 100:.2f}%" for s in steps),
        ),
    ]
    return ScenarioVerdict(scenario="N-3", passed=all(c.passed for c in checks), checks=checks)


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
