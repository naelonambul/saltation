import asyncio
from datetime import date, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock

import httpx
import pytest

from saltation_gen.config.stage6 import ScheduleConfig
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6 import (
    Due,
    RunConfig,
    _begin_live,
    _checked_live_time,
    _due,
    _index_scenarios,
    _live_anchor,
    _scenario_work,
    _timing,
    _waves,
)
from saltation_gen.drivers.stage6_schedule import build_schedule
from saltation_gen.drivers.stage6_types import Episode, Step
from saltation_gen.truth.recovery import RecoveryError

DAY = date(2026, 1, 4)
AT = datetime(2026, 1, 4, 10, tzinfo=KST)


class Clock:
    def __init__(self):
        self.progress = {}
        self.bank = SimpleNamespace(
            clock=AsyncMock(return_value={"mode": "BACKFILL", "business_time": AT.isoformat()}),
            go_live=AsyncMock(side_effect=httpx.ReadTimeout("lost response")),
        )
        self.move_clock = AsyncMock()

    async def save(self, **fields):
        self.progress.update(fields)

    async def gate(self):
        pass


def item(episode, name, actor, peer=None, final=False):
    return Due(AT, episode, Step(name, "transfer", actor, peer=peer), final)


def test_adjacent_transfers_dispatch_in_separate_waves():
    episode = Episode("one", "E", AT, {"actor": "a", "receiver": "b"}, "annual")
    first = item(episode, "first", "a", "b")
    next_one = item(episode, "second", "b", "c")
    unrelated = item(episode, "third", "d", "e")
    assert list(_waves([first, next_one, unrelated], 3)) == [[first], [next_one, unrelated]]
    assert list(_waves([first, unrelated], 1)) == [[first], [unrelated]]


async def test_stop_drains_admitted_wave_and_resumes_from_checkpoint():
    stop = asyncio.Event()
    clock = Clock()
    journal = SimpleNamespace(credit=AsyncMock(), require_success=AsyncMock())
    episode = Episode("one", "E", AT, {"actor": "a", "receiver": "b"}, "annual")
    entered = asyncio.Event()
    release = asyncio.Event()
    seen = []

    async def execute(episode, step, now):
        seen.append(step.name)
        if step.name == "first":
            entered.set()
            await release.wait()

    executor = SimpleNamespace(execute=execute)
    due = [item(episode, "first", "a"), item(episode, "second", "a", final=True)]
    config = RunConfig(DAY)
    active = asyncio.create_task(_due(due, "backfill", clock, config, executor, journal, stop))
    await entered.wait()
    stop.set()
    assert not active.done()
    release.set()
    assert await active is False
    assert seen == ["first"]
    assert clock.progress["wave_offset"] == 1
    journal.credit.assert_not_awaited()
    stop.clear()
    assert await _due(due, "backfill", clock, config, executor, journal, stop) is True
    assert seen == ["first", "second"]
    journal.credit.assert_awaited_once_with("annual", "E", "one", "one")


async def test_switch_response_loss_observes_live_without_reissuing():
    now = datetime.now(KST)
    planned = now - timedelta(milliseconds=1)
    if planned.date() != now.date():
        pytest.skip("live cutover test straddled midnight")
    clock = Clock()
    stop = asyncio.Event()
    with pytest.raises(httpx.ReadTimeout):
        await _begin_live(clock, planned, stop)
    assert clock.progress["live_intended"] is True
    assert "live_started" not in clock.progress
    clock.bank.clock.side_effect = [
        {"mode": "BACKFILL", "business_time": planned.isoformat()},
        {
            "mode": "LIVE",
            "business_time": (now + timedelta(hours=2)).isoformat(),
            "live_started_at": now.isoformat(),
        },
    ]
    assert await _begin_live(clock, planned, stop) is True
    assert clock.progress["live_started"] == now.isoformat()
    clock.bank.go_live.assert_awaited_once()


def test_d_carry_in_actions_span_three_dates_with_live_only_completion():
    people = ["a", "b", "c", "d"]
    config = ScheduleConfig(len(people), annual={}, live={"D": 2}, normal=False)
    cutover = AT + timedelta(days=2)
    schedule = build_schedule(config, 5, DAY, 3, cutover, people)
    by_date = _index_scenarios(schedule)
    planned = {day: _scenario_work(day, episodes) for day, episodes in by_date.items()}
    assert set(planned) == {DAY, DAY + timedelta(days=1), cutover.date()}
    assert not any(item.final for day, items in planned.items() if day < cutover.date() for item in items)
    assert sum(item.final for item in planned[cutover.date()]) == 2
    assert {item.episode.window for items in planned.values() for item in items} == {"live"}


def test_single_terminal_serializes_session_start_and_cash_pairs():
    episode = Episode("one", "D", AT, {"actor": "a", "second": "b"}, "annual")
    actions = [
        Due(AT, episode, Step(name, op, actor), False)
        for name, op, actor in [
            ("a_start", "atm_start", "a"),
            ("a_cash", "atm_deposit", "a"),
            ("b_start", "atm_start", "b"),
            ("b_cash", "atm_deposit", "b"),
        ]
    ]
    assert list(_waves(actions, 8)) == [[item] for item in actions]


@pytest.mark.parametrize("invalid", [None, "broken", "2026-01-04T10:00:00"])
def test_live_start_rejects_missing_malformed_or_naive_value(invalid):
    observed = {"mode": "LIVE", "business_time": AT.isoformat()}
    if invalid is not None:
        observed["live_started_at"] = invalid
    with pytest.raises(RecoveryError, match="valid committed LIVE start"):
        _live_anchor(observed, AT)


async def test_live_start_rejects_changed_anchor_on_resume():
    clock = Clock()
    clock.progress["live_started"] = AT.isoformat()
    clock.bank.clock.return_value = {
        "mode": "LIVE",
        "business_time": (AT + timedelta(minutes=2)).isoformat(),
        "live_started_at": (AT + timedelta(minutes=1)).isoformat(),
    }
    with pytest.raises(RecoveryError, match="differs from the recorded checkpoint"):
        await _begin_live(clock, AT, asyncio.Event())
    clock.bank.go_live.assert_not_awaited()


@pytest.mark.parametrize("case", ["changed", "missing", "expired"])
async def test_live_wave_checks_original_anchor_and_expiration_before_dispatch(case):
    start = AT.replace(hour=23, minute=59, second=55)
    deadline = start + timedelta(seconds=10)
    actual = deadline if case == "expired" else start + timedelta(seconds=6)
    observed = {"mode": "LIVE", "business_time": actual.isoformat(), "live_started_at": start.isoformat()}
    assert _checked_live_time(observed, start).date() != start.date()
    if case == "changed":
        observed["live_started_at"] = (start + timedelta(seconds=1)).isoformat()
    if case == "missing":
        del observed["live_started_at"]
    clock = Clock()
    clock.bank.clock.return_value = observed
    planned = deadline - timedelta(seconds=1)
    episode = Episode("one", "F", planned, {"actor": "a"}, "live")
    work = Due(planned, episode, Step("login", "login", "a"), True)
    executor = SimpleNamespace(execute=AsyncMock())
    with pytest.raises(RecoveryError, match="checkpoint|valid committed|expired"):
        await _due(
            [work],
            "live",
            clock,
            RunConfig(DAY, live_seconds=10),
            executor,
            SimpleNamespace(),
            asyncio.Event(),
            start,
        )
    executor.execute.assert_not_awaited()


@pytest.mark.parametrize("hour", [2, 10, 14, 23])
def test_default_live_prerequisites_fit_before_final_backfill_midnight(hour):
    config = ScheduleConfig()
    run = RunConfig(DAY)
    backfill_end = datetime.combine(DAY + timedelta(days=run.days), datetime.min.time(), KST)
    cutover = backfill_end.replace(hour=hour)
    people = [f"C{i:08d}" for i in range(config.population)]
    schedule = build_schedule(config, 5, DAY, run.days, cutover, people)
    _timing(schedule, run, cutover, backfill_end)


@pytest.mark.parametrize("invalid", [None, "", "broken"])
async def test_corrupt_saved_anchor_is_not_replaced(invalid):
    clock = Clock()
    clock.progress.update(live_intended=True, live_started=invalid)
    clock.bank.clock.return_value = {
        "mode": "LIVE",
        "business_time": AT.isoformat(),
        "live_started_at": AT.isoformat(),
    }
    with pytest.raises(RecoveryError, match="recorded LIVE start is malformed"):
        await _begin_live(clock, AT, asyncio.Event())
    assert clock.progress["live_started"] == invalid
    clock.bank.go_live.assert_not_awaited()


async def test_due_waits_for_oracle_anchor_when_bank_wall_clock_is_behind():
    clock = Clock()
    stop = asyncio.Event()
    clock.bank.clock.return_value = {
        "mode": "LIVE",
        "business_time": (AT - timedelta(milliseconds=250)).isoformat(),
        "live_started_at": AT.isoformat(),
    }
    prior = AT - timedelta(seconds=1)
    episode = Episode("late", "normal_card", prior, {"actor": "a"}, "live")
    due = Due(prior, episode, Step("card", "card", "a"), True)
    executor = SimpleNamespace(execute=AsyncMock())
    running = asyncio.create_task(
        _due([due], "live", clock, RunConfig(DAY), executor, SimpleNamespace(), stop, AT)
    )
    await asyncio.sleep(0)
    executor.execute.assert_not_awaited()
    clock.bank.clock.return_value["business_time"] = AT.isoformat()
    assert await running
    executor.execute.assert_awaited_once_with(episode, due.step, AT)


async def test_chunked_normal_backfill_skips_completed_waves_before_clock_advance():
    at1 = AT
    at2 = AT + timedelta(seconds=12)
    ep1 = Episode("norm-1", "normal_card", at1, {"actor": "a"}, "annual")
    ep2 = Episode("norm-2", "normal_card", at2, {"actor": "b"}, "annual")
    due = [
        Due(at1, ep1, Step("card", "card", "a"), True),
        Due(at2, ep2, Step("card", "card", "b"), True),
    ]
    clock = Clock()
    clock.progress["current"] = at1.isoformat()
    dispatched = []
    clock_moves = []

    async def move_clock(target, *, gated=False):
        clock_moves.append(target)
        clock.progress["current"] = target.isoformat()

    clock.move_clock = AsyncMock(side_effect=move_clock)

    # Simulate resume after wave 1 already succeeded and clock already moved to at2,
    # before wave_offset was persisted at the end of the chunk.
    prepared_rows = {
        ("norm-1", "card"): {"state": "succeeded"},
        ("norm-2", "card"): {"state": "prepared"},
    }

    async def prepare_chunk(items):
        return prepared_rows

    async def execute_wave(wave_items, prepared_map):
        for ep, step, at in wave_items:
            dispatched.append((ep.id, step.name, at))
            prepared_map[(ep.id, step.name)]["state"] = "succeeded"

    executor = SimpleNamespace(prepare_chunk=prepare_chunk, execute_wave=execute_wave)
    assert await _due(due, "backfill", clock, RunConfig(DAY), executor, SimpleNamespace(), asyncio.Event())
    # Wave 1 was skipped before clock advance (no backward clock move to at1!).
    assert clock_moves == [at2]
    assert dispatched == [("norm-2", "card", at2)]
    assert clock.progress["wave_offset"] == 2
