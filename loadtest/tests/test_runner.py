from pathlib import Path

import httpx

from saltation_loadtest.client import Actor, LoadClient
from saltation_loadtest.runner import compute_success_rate, run_open_loop
from saltation_loadtest.scenarios import write_evidence_report
from saltation_loadtest.thresholds import evaluate_n1

EP = 'endpoint="/v2.0/transfer/withdraw/fin_num"'
LK = 'statement="ledger.lock_accounts"'


def test_compute_success_rate_excludes_business_422() -> None:
    assert compute_success_rate({200: 999, 422: 50, 503: 1}) == 0.999
    assert compute_success_rate({200: 100, 422: 10}) == 1.0


async def test_open_loop_runner_with_mock_transport(tmp_path: Path) -> None:
    seen = {"transfers": 0}

    def bank_handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v2.0/transfer/withdraw/fin_num":
            seen["transfers"] += 1
            return httpx.Response(200, json={"rsp_code": "A0000"})
        return httpx.Response(404)

    def metrics_handler(_req: httpx.Request) -> httpx.Response:
        c = seen["transfers"]
        body = "\n".join(
            [
                f'bank_http_request_duration_seconds_bucket{{{EP},le="0.01"}} {c}',
                f'bank_http_request_duration_seconds_bucket{{{EP},le="+Inf"}} {c}',
                f"bank_http_request_duration_seconds_count{{{EP}}} {c}",
                f"bank_http_request_duration_seconds_sum{{{EP}}} {c * 0.005}",
                f'bank_db_statement_duration_seconds_bucket{{{LK},le="0.01"}} {c}',
                f'bank_db_statement_duration_seconds_bucket{{{LK},le="+Inf"}} {c}',
                f"bank_db_statement_duration_seconds_count{{{LK}}} {c}",
                f"bank_db_statement_duration_seconds_sum{{{LK}}} {c * 0.002}",
            ]
        )
        return httpx.Response(200, text=body)

    client = LoadClient(
        "http://bank.test",
        "http://metrics.test",
        bank_transport=httpx.MockTransport(bank_handler),
        metrics_transport=httpx.MockTransport(metrics_handler),
    )
    a1 = Actor(
        "L1", "부하1", "01900000001", "pw", "99890000001", "FIN1", "tok1", "198.51.100.1", "KR-11", "d1"
    )
    a2 = Actor(
        "L2", "부하2", "01900000002", "pw", "99890000002", "FIN2", "tok2", "198.51.100.2", "KR-11", "d2"
    )

    try:
        res = await run_open_loop(
            client,
            target_tps=20,
            duration_seconds=0.5,
            pair_picker=lambda _seq: (a1, a2),
        )
    finally:
        await client.close()

    assert res.total_sent == 10
    assert res.non_business_success_rate == 1.0
    assert res.server is not None
    assert res.server.transfer_count == 10

    verdict = evaluate_n1(
        p50_ms=res.server.transfer_p50_ms,
        p95_ms=res.server.transfer_p95_ms,
        p99_ms=res.server.transfer_p99_ms,
        lock_wait_p95_ms=res.server.lock_wait_p95_ms,
        success_rate=res.non_business_success_rate,
    )
    assert verdict.passed
    jp, mp = write_evidence_report(tmp_path, "n1", {"total_sent": res.total_sent}, verdict)
    assert jp.exists() and mp.exists()
