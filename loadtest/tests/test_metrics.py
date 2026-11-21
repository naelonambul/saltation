from saltation_loadtest.metrics import (
    MetricsSnapshot,
    classify_limiting_resource,
    histogram_quantile,
    summarize_window,
)

TX = 'listener="generator",endpoint="/v2.0/transfer/withdraw/fin_num",status="200"'
LK = 'statement="ledger.lock_accounts"'

SAMPLE_BEFORE = "\n".join(
    [
        "# TYPE bank_http_request_duration_seconds histogram",
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.01"}} 10',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.05"}} 20',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.1"}} 20',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="+Inf"}} 20',
        f"bank_http_request_duration_seconds_sum{{{TX}}} 0.4",
        f"bank_http_request_duration_seconds_count{{{TX}}} 20",
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="0.01"}} 20',
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="0.05"}} 20',
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="+Inf"}} 20',
        f"bank_db_statement_duration_seconds_sum{{{LK}}} 0.1",
        f"bank_db_statement_duration_seconds_count{{{LK}}} 20",
        "bank_deadlock_retries_total 0",
        "bank_outbox_full_total 0",
        "bank_outbox_relay_errors_total 0",
        "bank_outbox_undelivered 0",
        "bank_daily_close_mismatches 0",
    ]
)

SAMPLE_AFTER = "\n".join(
    [
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.01"}} 60',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.05"}} 115',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="0.1"}} 120',
        f'bank_http_request_duration_seconds_bucket{{{TX},le="+Inf"}} 120',
        f"bank_http_request_duration_seconds_sum{{{TX}}} 2.4",
        f"bank_http_request_duration_seconds_count{{{TX}}} 120",
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="0.01"}} 115',
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="0.05"}} 120',
        f'bank_db_statement_duration_seconds_bucket{{{LK},le="+Inf"}} 120',
        f"bank_db_statement_duration_seconds_sum{{{LK}}} 0.6",
        f"bank_db_statement_duration_seconds_count{{{LK}}} 120",
        "bank_deadlock_retries_total 0",
        "bank_outbox_full_total 0",
        "bank_outbox_relay_errors_total 0",
        "bank_outbox_undelivered 12",
        "bank_daily_close_mismatches 0",
    ]
)


def test_histogram_quantile_interpolation() -> None:
    buckets = {0.01: 50.0, 0.05: 95.0, 0.10: 100.0, float("inf"): 100.0}
    assert histogram_quantile(0.50, buckets) == 0.01
    assert histogram_quantile(0.95, buckets) == 0.05
    p99 = histogram_quantile(0.99, buckets)
    assert p99 is not None and 0.089 < p99 < 0.091


def test_summarize_window_deltas() -> None:
    s = summarize_window(MetricsSnapshot.parse(SAMPLE_BEFORE), MetricsSnapshot.parse(SAMPLE_AFTER))
    assert s.transfer_count == 100
    assert s.transfer_p50_ms is not None and abs(s.transfer_p50_ms - 10.0) < 0.01
    assert s.transfer_p95_ms is not None and abs(s.transfer_p95_ms - 50.0) < 0.01
    assert s.lock_wait_count == 100
    assert s.lock_wait_p95_ms is not None and abs(s.lock_wait_p95_ms - 10.0) < 0.01
    assert s.outbox_undelivered_end == 12
    assert s.limiting_resource == "none_saturated"


def test_classify_limiting_resource() -> None:
    assert classify_limiting_resource(400.0, 200.0, 0, 0) == "row_lock_wait"
    assert classify_limiting_resource(400.0, 20.0, 0, 0) == "db_pool_or_cpu"
    assert classify_limiting_resource(100.0, 10.0, 0, 1) == "outbox_tablespace"
