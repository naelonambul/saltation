import asyncio
import re
from copy import deepcopy
from dataclasses import replace
from datetime import date, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock

import httpx
import pytest
from test_stage6_journal import Database

from saltation_gen.client.bank import BankClient
from saltation_gen.config.stage6 import ScheduleConfig
from saltation_gen.drivers import stage6, stage6_identity
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6 import (
    Due,
    RunConfig,
    _begin_live,
    _checked_live_time,
    _due,
    _index_scenarios,
    _live_anchor,
    _run_phases,
    _scenario_work,
    _timing,
    _waves,
)
from saltation_gen.drivers.stage6_identity import build_schedule_identity
from saltation_gen.drivers.stage6_schedule import Schedule, build_schedule
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


@pytest.fixture
async def identity_run(monkeypatch):
    database = Database()
    truth = SimpleNamespace(runs=database["runs"], roster=database["roster"], labels=database["labels"])
    config = ScheduleConfig(4, annual={"G": 1}, live={"D": 1}, normal=False)
    run = RunConfig(DAY, days=2)
    first = AT.replace(hour=0)
    requests = []

    def respond(request):
        requests.append(request.url.path)
        if request.url.path == "/internal/v1/outbox":
            return httpx.Response(200, json={"undelivered": 0})
        assert request.url.path == "/internal/v1/clock"
        return httpx.Response(200, json={"mode": "BACKFILL", "business_time": first.isoformat()})

    async def seed_world(*args, **kwargs):
        truth.roster.rows.extend(
            {
                "customer_no": customer,
                "protected": True,
                "account_no": f"account-{customer}",
                "phone": f"phone-{customer}",
                "fintech_use_num": f"fintech-{customer}",
                "card_no": f"card-{customer}",
            }
            for customer in stage6.world_customer_numbers(config.population)
        )

    bank = BankClient("http://synthetic", transport=httpx.MockTransport(respond))
    monkeypatch.setattr(stage6, "seed_world", seed_world)
    monkeypatch.delenv("SALTATION_CODE_REVISION", raising=False)

    async def invoke(*, resume=False, live_at=None, normal=False, stop=None):
        if stop is None:
            stop = asyncio.Event()
            stop.set()
        return await stage6.run_stage6(
            bank,
            truth,
            "identity-run",
            7,
            replace(config, annual={} if normal else config.annual, normal=normal),
            replace(run, live_at=live_at),
            "live" if live_at else "backfill",
            stop,
            resume=resume,
        )

    try:
        yield invoke, truth.runs.rows, requests
    finally:
        await bank.close()


async def test_resume_keeps_identity_and_updates_code_revision(identity_run, monkeypatch):
    invoke, rows, requests = identity_run
    monkeypatch.setenv("SALTATION_CODE_REVISION", "  original  ")
    first = await invoke()
    assert first["status"] == "stopped"
    assert first["progress"]["seed_state"] == "seeded"
    assert rows[0]["provenance"] == {"code_revision": "original"}
    inputs = deepcopy(rows[0]["inputs"])
    assert inputs["version"] == 2
    assert inputs["workload_version"] == 3
    assert inputs["schedule_digest"] == inputs["backfill_schedule_digest"]
    assert inputs["planned_counts"]["backfill"]["episodes"] == {"G": 1}
    assert "code_revision" not in inputs

    monkeypatch.setenv("SALTATION_CODE_REVISION", " next ")
    assert await invoke(resume=True) == first
    assert rows[0]["inputs"] == inputs
    assert rows[0]["provenance"] == {"code_revision": "next"}
    assert requests == ["/internal/v1/clock"] * 2


@pytest.mark.parametrize("revision", ["https://user:secret@example.test/repo", "a" * 81])
async def test_invalid_code_revision_is_rejected_before_claim(identity_run, monkeypatch, revision):
    invoke, rows, requests = identity_run
    monkeypatch.setenv("SALTATION_CODE_REVISION", revision)
    with pytest.raises(RecoveryError, match="^invalid SALTATION_CODE_REVISION format$"):
        await invoke()
    assert rows == requests == []


async def test_live_attach_rejects_pre_boundary_d_steps_before_bank_http(identity_run):
    invoke, rows, requests = identity_run
    await invoke()
    rows[0]["progress"]["backfill_finished"] = True
    stored_run = deepcopy(rows[0])
    with pytest.raises(RecoveryError, match="inputs differ") as error:
        await invoke(resume=True, live_at=AT + timedelta(days=2))
    match = re.search(
        r"backfill_schedule_digest \(stored='(sha256:[0-9a-f]{64})', computed='(sha256:[0-9a-f]{64})'\)",
        str(error.value),
    )
    assert match
    assert match[1] == stored_run["inputs"]["backfill_schedule_digest"]
    assert match[1] != match[2]
    assert rows[0] == stored_run
    assert requests == ["/internal/v1/clock"] * 2


@pytest.mark.parametrize("change", ["amount", "empty", "missing_digest", "offset"])
async def test_resume_rejects_changed_normal_day_before_bank_http(identity_run, monkeypatch, change):
    invoke, rows, requests = identity_run
    stop = asyncio.Event()

    def normal_episodes(schedule, config, seed, day, first):
        assert config.normal
        actor = next(iter(schedule.profiles))
        start = AT.replace(hour=0)
        return [
            Episode(
                f"normal_card:{day}:{i}",
                "normal_card",
                start + timedelta(seconds=i),
                {"actor": actor},
                "annual",
            )
            for i in range(2)
        ]

    async def prepare_chunk(items):
        return {(episode.id, step.name): {"state": "prepared"} for episode, step, _ in items}

    async def execute_wave(items, prepared):
        stop.set()

    monkeypatch.setattr(stage6, "normal_episodes", normal_episodes)
    monkeypatch.setattr(stage6, "normal_amount", lambda *args: 100)
    monkeypatch.setattr(
        stage6,
        "Stage6Executor",
        lambda *args: SimpleNamespace(prepare_chunk=prepare_chunk, execute_wave=execute_wave),
    )
    stopped = await invoke(normal=True, stop=stop)
    assert stopped["status"] == "stopped"
    assert stopped["progress"]["wave_date"] == f"backfill:{DAY}"
    assert stopped["progress"]["wave_offset"] == 1
    assert re.fullmatch(r"sha256:[0-9a-f]{64}", stopped["progress"]["wave_day_digest"])
    if change == "amount":
        monkeypatch.setattr(stage6, "normal_amount", lambda *args: 101)
    elif change == "empty":
        monkeypatch.setattr(stage6, "normal_episodes", lambda *args: [])
    elif change == "missing_digest":
        del rows[0]["progress"]["wave_day_digest"]
    else:
        rows[0]["progress"]["wave_offset"] = 3
    requests.clear()
    stop.clear()
    message = "offset" if change == "offset" else "digest differs"
    with pytest.raises(RecoveryError, match=f"wave checkpoint {message} for backfill:{DAY}"):
        await invoke(resume=True, normal=True, stop=stop)
    assert requests == []


@pytest.mark.parametrize(
    "change", ["workload_version", "legacy", "step", "offset", "expect_declined", "wave_order", "final_step"]
)
async def test_resume_rejects_identity_changes_before_bank_http(identity_run, monkeypatch, change):
    invoke, rows, requests = identity_run
    assert (await invoke())["status"] == "stopped"
    assert "provenance" not in rows[0]
    original = deepcopy(rows[0]["inputs"])
    if change == "workload_version":
        monkeypatch.setattr(stage6_identity, "WORKLOAD_VERSION", 4)
    elif change == "legacy":
        legacy = rows[0]["inputs"]
        legacy["version"] = 1
        for key in ("workload_version", "schedule_digest", "backfill_schedule_digest", "planned_counts"):
            del legacy[key]
    else:
        original_steps = stage6.steps

        def changed_steps(episode):
            actions = original_steps(episode)
            if change == "wave_order":
                index = next(i for i, step in enumerate(actions) if step.op == "atm_start")
                actions[index : index + 2] = reversed(actions[index : index + 2])
            elif change == "final_step":
                assert actions[-1].offset != actions[-2].offset
                actions[-2:] = reversed(actions[-2:])
            else:
                index = next(i for i, step in enumerate(actions) if step.args.get("expect_declined"))
                target = actions[index]
                actions[index] = {
                    "step": replace(target, name=f"{target.name}_changed"),
                    "offset": replace(target, offset=target.offset + 1),
                    "expect_declined": replace(target, args=target.args | {"expect_declined": False}),
                }[change]
            return actions

        monkeypatch.setattr(stage6, "steps", changed_steps)

    stored_run = deepcopy(rows[0])
    with pytest.raises(RecoveryError, match="inputs differ") as error:
        await invoke(resume=True)
    message = str(error.value)
    if change == "workload_version":
        assert "workload_version (stored=3, computed=4)" in message
    elif change == "legacy":
        assert "version (stored=1, computed=2)" in message
        assert "workload_version (stored=None, computed=3)" in message
        assert f"schedule_digest (stored=None, computed={original['schedule_digest']!r})" in message
    else:
        for field in ("schedule_digest", "backfill_schedule_digest"):
            match = re.search(
                rf"\b{field} \(stored='(sha256:[0-9a-f]{{64}})', computed='(sha256:[0-9a-f]{{64}})'\)",
                message,
            )
            assert match
            assert match[1] == original[field]
            assert match[1] != match[2]
    assert rows[0] == stored_run
    assert requests == ["/internal/v1/clock"] * 2


def test_schedule_identity_canonicalizes_mappings_and_separates_live_work():
    annual = Episode("annual", "A", AT, {"actor": "a", "receiver": "b"}, "annual")
    live_at = AT + timedelta(days=1)
    live = Episode("live", "D", live_at, {"actor": "c"}, "live", carry_in=True)
    action = Step("send", "transfer", "a", args={"amount": 10, "options": {"y": 2, "x": 1}}, peer="b")
    later = Step("login", "login", "a", offset=5)
    live_login = Step("login", "login", "c")
    schedule = Schedule([annual, live], {"a", "c"}, {"a": "working", "c": "older"}, set())
    backfill = [(annual, action, AT), (annual, later, AT + timedelta(seconds=5))]
    planned = {DAY: backfill, live_at.date(): [(live, live_login, live_at)]}
    identity = build_schedule_identity(schedule, planned, live_at)
    reordered = dict(reversed(planned.items()))
    reordered[DAY] = [
        (
            replace(annual, roles={"receiver": "b", "actor": "a"}),
            replace(action, args={"options": {"x": 1, "y": 2}, "amount": 10}),
            AT,
        ),
        backfill[1],
    ]
    assert build_schedule_identity(schedule, reordered, live_at) == identity
    assert identity["planned_counts"] == {
        "backfill": {"episodes": {"A": 1}, "operations": 2},
        "live": {"episodes": {"D": 1}, "operations": 1},
    }

    changed_live = planned | {live_at.date(): [(live, live_login, live_at + timedelta(seconds=1))]}
    new_identity = build_schedule_identity(schedule, changed_live, live_at)
    assert new_identity["schedule_digest"] != identity["schedule_digest"]
    assert new_identity["backfill_schedule_digest"] == identity["backfill_schedule_digest"]
    changed_dormant = build_schedule_identity(replace(schedule, dormant={"c"}), planned, live_at)
    assert changed_dormant["schedule_digest"] != identity["schedule_digest"]
    assert changed_dormant["backfill_schedule_digest"] != identity["backfill_schedule_digest"]
    profiles = dict(reversed(schedule.profiles.items()))
    reordered_profiles = build_schedule_identity(replace(schedule, profiles=profiles), planned, live_at)
    assert reordered_profiles["schedule_digest"] != identity["schedule_digest"]
    assert reordered_profiles["backfill_schedule_digest"] != identity["backfill_schedule_digest"]


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


@pytest.mark.parametrize("has_checkpoint", [False, True])
async def test_live_resume_checks_checkpoint_and_anchor_before_advancing_day(has_checkpoint):
    clock = Clock()
    clock.progress.update(backfill_finished=True, live_started=AT.isoformat())
    if has_checkpoint:
        clock.progress.update(wave_date=f"live:{DAY}", wave_offset=1)
    saved = deepcopy(clock.progress)
    clock.bank.clock.return_value = {
        "mode": "LIVE",
        "business_time": (AT + timedelta(minutes=2)).isoformat(),
        "live_started_at": (AT + timedelta(minutes=1)).isoformat(),
    }
    message = "wave checkpoint digest differs" if has_checkpoint else "differs from the recorded checkpoint"
    with pytest.raises(RecoveryError, match=message):
        await _run_phases(
            {},
            Schedule([], set(), {}, set()),
            ScheduleConfig(annual={}, live={}, normal=False),
            RunConfig(DAY, live_at=AT),
            7,
            "live",
            AT,
            AT,
            clock,
            SimpleNamespace(),
            SimpleNamespace(),
            asyncio.Event(),
        )
    assert clock.progress == saved
    assert clock.bank.clock.await_count == (0 if has_checkpoint else 1)


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


async def test_live_wave_skips_expired_normal_wave_without_failing():
    start = AT.replace(hour=23, minute=59, second=55)
    deadline = start + timedelta(seconds=10)
    observed = {"mode": "LIVE", "business_time": deadline.isoformat(), "live_started_at": start.isoformat()}
    clock = Clock()
    clock.bank.clock.return_value = observed
    planned = deadline - timedelta(seconds=1)
    episode = Episode("normal_transfer:2026-01-04:C1", "normal_transfer", planned, {"actor": "a"}, "annual")
    work = Due(planned, episode, Step("login", "login", "a"), True)
    executor = SimpleNamespace(execute=AsyncMock())
    assert await _due(
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


def test_waves_parallelize_distinct_regional_atm_terminals():
    ep1 = Episode("normal_atm:2026-01-04:C1", "normal_atm", AT, {"actor": "C1"}, "annual")
    ep2 = Episode("normal_atm:2026-01-04:C2", "normal_atm", AT, {"actor": "C2"}, "annual")
    a = Due(AT, ep1, Step("atm_start", "atm_start", "C1", args={"terminal_id": "ATM-KR-11-01"}), False)
    b = Due(AT, ep2, Step("atm_start", "atm_start", "C2", args={"terminal_id": "ATM-KR-26-01"}), False)
    c = Due(AT, ep2, Step("atm_start", "atm_start", "C3", args={"terminal_id": "ATM-KR-11-01"}), False)
    assert list(_waves([a, b], 8)) == [[a, b]]
    assert list(_waves([a, c], 8)) == [[a], [c]]
