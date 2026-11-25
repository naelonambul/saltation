from collections import Counter
from dataclasses import replace
from datetime import UTC, date, datetime, time, timedelta

import pytest

from saltation_gen.config.card import CARD_KRW_ANCHORS, NORMAL_CARD_ANNUAL_CAP
from saltation_gen.config.stage6 import BANKING_KRW_ANCHORS, LOOKALIKE_KINDS, ScheduleConfig
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6_schedule import (
    build_schedule,
    estimate_normal_requests,
    normal_amount,
    normal_episodes,
)
from saltation_gen.personas.profiles import PROFILES, REQUESTS_PER_EPISODE

FIRST = date(2024, 1, 1)
SEED = 62


def roster(size):
    return [f"customer-{index}" for index in range(size)]


def counts(episodes, window):
    result = Counter()
    for episode in episodes:
        if episode.window != window:
            continue
        if episode.kind == "BC":
            result["B"] += 1 + ("victim_2" in episode.roles)
            result["C"] += 1
        else:
            result["lookalike" if episode.kind in LOOKALIKE_KINDS else episode.kind] += 1
    return result


def test_default_quota_reservations_and_request_estimate():
    config = ScheduleConfig()
    customers = roster(config.population)
    cutover = datetime.combine(FIRST + timedelta(days=365), time(11), KST)
    schedule = build_schedule(config, SEED, FIRST, 365, cutover, customers)

    assert counts(schedule.episodes, "annual") == {
        "A": 200,
        "B": 200,
        "C": 100,
        "D": 100,
        "E": 300,
        "F": 300,
        "G": 200,
        "lookalike": 2_000,
    }
    assert counts(schedule.episodes, "live") == {
        "A": 2,
        "B": 2,
        "C": 2,
        "D": 2,
        "E": 2,
        "F": 2,
        "G": 2,
        "lookalike": 7,
    }
    assert set(schedule.profiles) == set(customers)
    assert set(schedule.profiles.values()) == set(PROFILES)
    assert {episode.kind for episode in schedule.episodes}.issuperset(LOOKALIKE_KINDS)
    roles = [customer for episode in schedule.episodes for customer in episode.roles.values()]
    assert len(roles) == len(schedule.reserved)
    assert set(roles) == schedule.reserved
    assert len({episode.id for episode in schedule.episodes}) == len(schedule.episodes)

    bc = [episode for episode in schedule.episodes if episode.kind == "BC"]
    assert schedule.dormant == {episode.roles["mule"] for episode in bc}
    assert len(schedule.dormant) == 102
    assert {ep.roles["mule"] for ep in bc if ep.window == "live"}.isdisjoint(
        ep.roles["mule"] for ep in bc if ep.window == "annual"
    )
    for episode in bc:
        assert set(episode.roles) in ({"victim_1", "mule"}, {"victim_1", "victim_2", "mule"})
        assert all(
            schedule.profiles[customer] == "older"
            for role, customer in episode.roles.items()
            if role.startswith("victim_")
        )

    for kind in ("A", "E", "purchase"):
        assert all(set(ep.roles) == {"actor", "receiver"} for ep in schedule.episodes if ep.kind == kind)
    for kind in ("D", "household"):
        assert all(set(ep.roles) == {"actor", "second"} for ep in schedule.episodes if ep.kind == kind)
    for kind in ("F", "G", "travel", "trip", "online", "new_phone"):
        assert all(set(ep.roles) == {"actor"} for ep in schedule.episodes if ep.kind == kind)

    assert estimate_normal_requests(schedule, config) == pytest.approx(8_550_000, rel=0.05)
    eligible_estimate = estimate_normal_requests(schedule, config, include_reserved=False)
    assert eligible_estimate == pytest.approx(8_550_000, rel=0.05)
    assert eligible_estimate < estimate_normal_requests(schedule, config)


def test_reference_counts_and_override_determinism():
    config = ScheduleConfig(amplification=1)
    customers = roster(250)
    schedule = build_schedule(config, SEED, FIRST, 365, None, customers)
    assert counts(schedule.episodes, "annual") == {
        "A": 10,
        "B": 10,
        "C": 5,
        "D": 5,
        "E": 15,
        "F": 15,
        "G": 10,
        "lookalike": 100,
    }
    assert counts(schedule.episodes, "live") == {}
    assert schedule == build_schedule(config, SEED, FIRST, 365, None, customers[::-1])
    assert schedule != build_schedule(config, SEED + 1, FIRST, 365, None, customers)

    override = replace(config, annual={"B": 3, "C": 2}, live={})
    short = build_schedule(override, SEED, FIRST, 1, None, customers)
    assert counts(short.episodes, "annual") == {"B": 3, "C": 2}
    assert sorted(len(ep.roles) for ep in short.episodes) == [2, 3]


def test_d_business_dates_and_cutover_boundary(monkeypatch):
    monkeypatch.setenv("SALTATION_MAINTENANCE_MINUTES", "0")
    config = ScheduleConfig(annual={"D": 1}, live={"D": 2, "G": 1})
    cutover = datetime(2024, 1, 3, 15, tzinfo=UTC)
    schedule = build_schedule(config, SEED, FIRST, 3, cutover, roster(9))
    assert counts(schedule.episodes, "annual") == {"D": 1}
    assert counts(schedule.episodes, "live") == {"D": 2, "G": 1}
    for episode in schedule.episodes:
        assert episode.start.tzinfo == KST
        if episode.kind != "D":
            assert timedelta(0) <= episode.start - cutover < timedelta(seconds=30)
            continue

        dates = [(episode.start + timedelta(days=i)).date() for i in range(3)]
        assert dates == [episode.start.date() + timedelta(days=i) for i in range(3)]
        if episode.window == "annual":
            assert dates == [date(2024, 1, d) for d in (1, 2, 3)]
            assert not episode.carry_in
            assert episode.start + timedelta(seconds=172_829) < cutover
        else:
            assert episode.carry_in
            assert dates == [date(2024, 1, d) for d in (2, 3, 4)]
            final = episode.start + timedelta(seconds=172_800)
            assert timedelta(0) <= final - cutover < timedelta(seconds=30)
            assert cutover <= final + timedelta(seconds=29) < cutover + timedelta(hours=24)


def test_annual_longest_episode_has_room_before_cutover():
    config = ScheduleConfig(annual={"G": 1}, live={})
    for hour, minute, expected_date in ((11, 4, FIRST), (11, 5, FIRST + timedelta(days=1))):
        cutover = datetime(2024, 1, 2, hour, minute, tzinfo=KST)
        starts = [
            build_schedule(config, seed, FIRST, 2, cutover, roster(1)).episodes[0].start for seed in range(8)
        ]
        assert max(start.date() for start in starts) == expected_date
        assert all(start + timedelta(seconds=3_840) < cutover for start in starts)


def test_live_g_can_cross_midnight_and_d_only_override_is_supported(monkeypatch):
    monkeypatch.setenv("SALTATION_MAINTENANCE_MINUTES", "0")
    cutover = datetime(2024, 1, 3, 23, 59, 45, tzinfo=KST)
    g_only = ScheduleConfig(annual={}, live={"G": 1})
    (g,) = build_schedule(g_only, SEED, FIRST, 2, cutover, roster(1)).episodes
    assert cutover <= g.start < cutover + timedelta(seconds=30)
    g_final = g.start + timedelta(seconds=3_840)
    assert g_final.date() > cutover.date()
    assert g_final < cutover + timedelta(hours=24)

    d_only = replace(g_only, live={"D": 2})
    schedule = build_schedule(d_only, SEED, FIRST, 2, cutover.replace(hour=12), roster(4))
    assert counts(schedule.episodes, "annual") == {}
    assert counts(schedule.episodes, "live") == {"D": 2}
    assert all(episode.carry_in for episode in schedule.episodes)


@pytest.mark.parametrize(
    ("config", "days", "cutover", "size", "message"),
    [
        (ScheduleConfig(annual={"D": 1}), 2, None, 10, "three business dates"),
        (ScheduleConfig(annual={"D": 1}), 0, None, 10, "no eligible business date"),
        (
            ScheduleConfig(annual={}, live={"D": 2}),
            1,
            datetime(2024, 1, 2, 10, tzinfo=KST),
            10,
            "two preparation dates",
        ),
        (
            ScheduleConfig(annual={}, live={"D": 2}),
            2,
            datetime(2024, 1, 3, 23, 59, 45, tzinfo=KST),
            10,
            "near KST midnight",
        ),
        (
            ScheduleConfig(annual={}, live={"G": 1}),
            1,
            datetime(2024, 1, 3, 10, tzinfo=KST),
            10,
            "outside the schedule date range",
        ),
        (ScheduleConfig(annual={"B": 2, "C": 3}), 1, None, 10, "B/C quotas"),
        (ScheduleConfig(annual={"B": 3, "C": 1}), 1, None, 10, "B/C quotas"),
        (ScheduleConfig(annual={"E": 2}), 1, None, 3, "distinct customers"),
        (ScheduleConfig(annual={"B": 2, "C": 1}), 1, None, 4, "older customers"),
        (ScheduleConfig(amplification=0), 1, None, 10, "amplification"),
        (ScheduleConfig(annual={"A": -1}), 1, None, 10, "nonnegative integers"),
    ],
)
def test_unschedulable(config, days, cutover, size, message):
    with pytest.raises(ValueError, match=message):
        build_schedule(config, SEED, FIRST, days, cutover, roster(size))


def test_normal_cadences_by_kst_calendar_year_and_request_counts():
    config = ScheduleConfig(annual={"B": 1, "C": 1}, live={})
    schedule = build_schedule(config, SEED, FIRST, 366, None, roster(17))
    assert list(normal_episodes(schedule, config, SEED, FIRST - timedelta(days=1), FIRST)) == []
    assert list(normal_episodes(schedule, replace(config, normal=False), SEED, FIRST, FIRST)) == []
    customer_year = Counter()
    request_count = 0
    for offset in range(366 + 365):
        day = FIRST + timedelta(days=offset)
        episodes = list(normal_episodes(schedule, config, SEED, day, FIRST))
        assert episodes == sorted(episodes, key=lambda episode: (episode.start, episode.id))

        due_subjects = set()
        for episode in episodes:
            assert episode.start.tzinfo == KST
            assert episode.start.date() == day
            actor = episode.roles["actor"]
            profile = PROFILES[schedule.profiles[actor]]
            assert episode.start.hour == profile.cadence(episode.kind).hour
            subjects = {actor}
            if episode.kind in {"normal_transfer", "normal_visit"}:
                assert episode.roles["receiver"] != actor
                subjects.add(episode.roles["receiver"])
            elif episode.kind == "normal_income":
                assert episode.roles["activity"] == profile.income_activity
                assert normal_amount(schedule, episode, SEED) == profile.income_krw
            assert subjects.isdisjoint(schedule.dormant | schedule.blocked_days.get(day, set()))
            for subject in subjects:
                key = (episode.start.replace(second=0), subject)
                assert key not in due_subjects
                due_subjects.add(key)
            customer_year[day.year, actor, episode.kind] += 1
            request_count += REQUESTS_PER_EPISODE[episode.kind]

    expected_requests = estimate_normal_requests(schedule, config, include_reserved=False)
    assert request_count == pytest.approx(2 * expected_requests, rel=0.01)
    for year in (2024, 2025):
        for actor, name in schedule.profiles.items():
            if actor not in schedule.reserved:
                transfer_total = (
                    customer_year[year, actor, "normal_transfer"] + customer_year[year, actor, "normal_visit"]
                )
                assert transfer_total == PROFILES[name].transfer.annual
                assert customer_year[year, actor, "normal_visit"] > 0
                assert customer_year[year, actor, "normal_transfer"] > 0
                for kind in ("normal_income", "normal_card", "normal_atm", "normal_login_balance"):
                    assert customer_year[year, actor, kind] == PROFILES[name].cadence(kind).annual
                assert customer_year[year, actor, "normal_card"] <= NORMAL_CARD_ANNUAL_CAP

    day = FIRST + timedelta(days=40)
    first_run = list(normal_episodes(schedule, config, SEED, day, FIRST))
    assert first_run == list(normal_episodes(schedule, config, SEED, day, FIRST))
    assert first_run != list(normal_episodes(schedule, config, SEED + 1, day, FIRST))


def test_amount_anchors_and_profile_contexts():
    class Percentile:
        def __init__(self, pct):
            self.pct = pct

        def random(self):
            return self.pct / 100

    for name, profile in PROFILES.items():
        for kind, anchors in (
            ("normal_transfer", BANKING_KRW_ANCHORS),
            ("normal_card", CARD_KRW_ANCHORS),
        ):
            for pct, amount in anchors[1:-1]:
                assert profile.amount(kind, Percentile(pct)) == amount

        contexts = {profile.context(SEED, "subject", str(i)) for i in range(100)}
        assert {device for device, _ in contexts} == {f"dev-subject-{device}" for device in profile.devices}
        assert len({region for _, region in contexts}) == profile.location_count
        assert 420 <= PROFILES[name].annual_requests <= 440


def test_normal_episodes_spread_across_hour_and_scenario_g_includes_declined_card():
    from saltation_gen.drivers.stage6_steps import steps
    from saltation_gen.drivers.stage6_types import Episode

    config = ScheduleConfig(population=120, amplification=1, annual={}, live={}, normal=True)
    customers = [f"C{i:08d}" for i in range(1, 121)]
    schedule = build_schedule(config, SEED, FIRST, 10, None, customers)
    minutes = {
        ep.start.minute
        for day_offset in range(10)
        for ep in normal_episodes(schedule, config, SEED, FIRST + timedelta(days=day_offset), FIRST)
    }
    assert len(minutes) >= 20, f"expected hour-wide minute spread, got {sorted(minutes)}"

    g_ep = Episode("G:test:C1", "G", datetime(2026, 1, 1, 10, 0, tzinfo=KST), {"actor": "C1"}, "annual")
    g_steps = {step.name: step for step in steps(g_ep)}
    assert "declined_card" in g_steps
    assert g_steps["declined_card"].args.get("expect_declined") is True
    assert g_steps["declined_card"].args["amount"] == 50_000


def test_live_monetary_steps_shift_out_of_maintenance_window(monkeypatch):
    from saltation_gen.drivers.stage6_schedule import MONETARY_OPS, in_maintenance_window
    from saltation_gen.drivers.stage6_steps import steps

    monkeypatch.setenv("SALTATION_MAINTENANCE_MINUTES", "30")
    config = ScheduleConfig(annual={}, live={"A": 1, "B": 1, "C": 1, "D": 1, "E": 1, "G": 1, "lookalike": 2})
    for cutover in (
        datetime(2024, 1, 5, 0, 5, tzinfo=KST),
        datetime(2024, 1, 5, 23, 0, tzinfo=KST),
    ):
        schedule = build_schedule(config, SEED, FIRST, 5, cutover, roster(40))
        for episode in schedule.episodes:
            if episode.window != "live":
                continue
            for action in steps(episode):
                if action.op in MONETARY_OPS:
                    at = episode.start + timedelta(seconds=action.offset)
                    assert not in_maintenance_window(at, 30), (
                        f"{episode.id}/{action.name} landed in maintenance: {at}"
                    )
