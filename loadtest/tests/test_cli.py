import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from saltation_loadtest import cli, verify_host, verify_oracle
from saltation_loadtest.thresholds import ScenarioVerdict


def result(tmp_path: Path) -> dict:
    return json.loads(next(tmp_path.glob("*-summary.json")).read_text())


@pytest.mark.parametrize(
    "args",
    [
        ["--qualify", "n1"],
        ["--qualify", "--smoke", "all"],
        ["--qualify", "--duration", "5", "all"],
        ["--qualify", "--steps", "50,25,100,200", "all"],
        ["--steps", "", "n3"],
    ],
)
async def test_invalid_invocation_is_incomplete(tmp_path: Path, args: list[str]) -> None:
    parsed = cli.build_parser().parse_args(["--report-dir", str(tmp_path), *args])
    assert await cli._run(parsed) == 2
    assert result(tmp_path)["outcome"] == "INCOMPLETE"


@pytest.mark.parametrize(
    "n5_value,host_gaps,expected", [(0, [], "PASS"), (1, [], "FAIL"), (0, ["gap"], "INCOMPLETE")]
)
async def test_qualification_requires_end_to_end_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, n5_value: int, host_gaps: list[str], expected: str
) -> None:
    class Client:
        def __init__(self, *_args):
            pass

        async def healthz(self):
            return True

        async def close(self):
            pass

    class Host:
        incomplete = []
        violations = []

        @classmethod
        async def start(cls):
            return cls()

        def bind_targets(self, **_targets):
            return True

        async def finish(self):
            return {"incomplete": host_gaps, "violations": []}

        async def close(self):
            pass

    class Oracle:
        @classmethod
        async def start(cls, _accounts):
            return cls()

        async def verify(self, _attempts):
            return {"daily_close_mismatches": n5_value, "duplicate_movements": 0, "f21_violations": 0}

    async def prepare(*_args, **_kwargs):
        return [SimpleNamespace(account_no="test")]

    from saltation_loadtest.runner import RunResult

    window = RunResult(50, 600, 0, 0, {}, 1.0, None, None, None)

    async def one(*_args, **_kwargs):
        return window, ScenarioVerdict("transfer", True)

    async def n3(*_args, **_kwargs):
        return [window], ScenarioVerdict("N-3", True)

    for module, name, value in (
        (cli, "LoadClient", Client),
        (cli, "prepare_actors", prepare),
        (cli, "run_n1", one),
        (cli, "run_n2", one),
        (cli, "run_n3", n3),
        (verify_oracle, "OracleVerifier", Oracle),
        (verify_host, "HostVerifier", Host),
    ):
        monkeypatch.setattr(module, name, value)

    args = cli.build_parser().parse_args(
        ["--report-dir", str(tmp_path), "--revision", "a" * 40, "--qualify", "all"]
    )
    assert await cli._run(args) == {"PASS": 0, "FAIL": 1, "INCOMPLETE": 2}[expected]
    assert result(tmp_path)["outcome"] == expected


def test_maintenance_window_conflict_detects_backfill_and_live_overlap() -> None:
    assert (
        cli.maintenance_window_conflict(
            {"mode": "BACKFILL", "business_time": "2026-01-02T00:10:00+09:00"}, 600.0, 30
        )
        is not None
    )
    assert (
        cli.maintenance_window_conflict(
            {"mode": "BACKFILL", "business_time": "2026-01-02T00:30:00+09:00"}, 600.0, 30
        )
        is None
    )
    assert (
        cli.maintenance_window_conflict(
            {"mode": "LIVE", "business_time": "2026-01-02T23:50:00+09:00"}, 1200.0, 30
        )
        is not None
    )
    assert (
        cli.maintenance_window_conflict(
            {"mode": "LIVE", "business_time": "2026-01-02T14:00:00+09:00"}, 1800.0, 30
        )
        is None
    )
    assert (
        cli.maintenance_window_conflict(
            {"mode": "LIVE", "business_time": "2026-01-02T00:05:00+09:00"}, 600.0, 0
        )
        is None
    )
