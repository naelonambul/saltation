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


def test_n3_saturation_above_100_tps() -> None:
    ok = evaluate_n3(
        [
            StepOutcome(25, 25.0, 40.0, 1.0, "none_saturated"),
            StepOutcome(50, 50.0, 80.0, 1.0, "none_saturated"),
            StepOutcome(100, 99.5, 210.0, 1.0, "none_saturated"),
            StepOutcome(200, 180.0, 450.0, 0.99, "db_pool_or_cpu"),
        ]
    )
    assert ok.passed

    bad = evaluate_n3(
        [
            StepOutcome(25, 25.0, 40.0, 1.0, "none_saturated"),
            StepOutcome(50, 50.0, 80.0, 1.0, "none_saturated"),
            StepOutcome(100, 95.0, 350.0, 1.0, "row_lock_wait"),
            StepOutcome(200, 120.0, 900.0, 0.95, "row_lock_wait"),
        ]
    )
    assert not bad.passed


def test_n5_correctness() -> None:
    assert evaluate_n5(daily_close_mismatches=0, duplicate_movements=0, f21_violations=0).passed
    assert not evaluate_n5(daily_close_mismatches=1, duplicate_movements=0, f21_violations=0).passed
