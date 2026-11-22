from saltation_loadtest.thresholds import (
    StepOutcome,
    evaluate_n1,
    evaluate_n2,
    evaluate_n3,
    evaluate_n5,
)


def test_n1_pass_and_fail() -> None:
    ok = evaluate_n1(p50_ms=20.0, p95_ms=120.0, p99_ms=400.0, lock_wait_p95_ms=15.0, success_rate=1.0)
    assert ok.passed
    bad = evaluate_n1(p50_ms=60.0, p95_ms=120.0, p99_ms=400.0, lock_wait_p95_ms=15.0, success_rate=1.0)
    assert not bad.passed


def test_n2_requires_200_senders() -> None:
    ok = evaluate_n2(p95_ms=250.0, success_rate=0.9995, sender_count=200)
    assert ok.passed
    bad = evaluate_n2(p95_ms=250.0, success_rate=1.0, sender_count=199)
    assert not bad.passed


def step(tps: int, p95_ms: float | None = 50, resource: str = "none_saturated") -> StepOutcome:
    return StepOutcome(tps, tps, p95_ms, 1.0, resource, 180.0, True, True, 1)


def test_n3_saturation_above_100_tps() -> None:
    steps = [step(25), step(50), step(100), step(200, 450.0, "db_pool")]
    assert evaluate_n3(steps).passed
    steps[2] = step(100, 350, "row_lock_wait")
    assert not evaluate_n3(steps).passed
    assert evaluate_n3(steps).evidence_complete


def test_n3_rejects_incomplete_steps() -> None:
    steps = [step(25), step(50), step(100), step(200)]
    assert evaluate_n3(steps).passed
    for invalid in ([], steps[:3], steps[::-1], [*steps[:3], step(100)]):
        assert not evaluate_n3(invalid).evidence_complete
    for invalid_last in (step(200, None), step(200, 450, "unknown")):
        assert not evaluate_n3([*steps[:3], invalid_last]).evidence_complete
    steps[-1].achieved_tps = 180
    assert not evaluate_n3(steps).evidence_complete


def test_n5_correctness() -> None:
    assert evaluate_n5(daily_close_mismatches=0, duplicate_movements=0, f21_violations=0).passed
    assert not evaluate_n5(daily_close_mismatches=1, duplicate_movements=0, f21_violations=0).passed


def test_n3_rejections_do_not_establish_capacity() -> None:
    steps = [step(25), step(50), step(100), step(200)]
    for window in steps:
        window.successes = 0
        window.success_rate = 0
    assert not evaluate_n3(steps).passed
