"""Deterministic KST plan for reserved scenarios and daily normal activity."""

import os
from calendar import isleap
from collections.abc import Iterator
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta
from random import Random

from saltation_gen.config.stage6 import ANNUAL_RATES, LIVE_RATES, LOOKALIKE_KINDS, ScheduleConfig
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6_steps import PRIOR_DAY, steps
from saltation_gen.drivers.stage6_types import Episode
from saltation_gen.personas.profiles import NORMAL_SCHEDULE_KINDS, PROFILES, stable_number

BACKFILL_HOUR = 10
LIVE_START_OFFSET = timedelta(seconds=1)
ANNUAL_PHASE_LENGTH = timedelta(minutes=65)
LIVE_D_MIDNIGHT_MARGIN = timedelta(seconds=30)
D_PREPARATION = timedelta(days=2)
MONETARY_OPS = frozenset(
    {"transfer", "inbound", "card", "atm_withdraw", "atm_deposit", "term_open", "term_close"}
)


def _maintenance_minutes() -> int:
    raw = os.environ.get("SALTATION_MAINTENANCE_MINUTES", "30").strip()
    if not raw:
        return 30
    minutes = int(raw)
    if not 0 <= minutes <= 60:
        raise ValueError(f"SALTATION_MAINTENANCE_MINUTES must be in 0..=60, got {minutes}")
    return minutes


def in_maintenance_window(instant: datetime, minutes: int | None = None) -> bool:
    window = _maintenance_minutes() if minutes is None else minutes
    if window <= 0:
        return False
    kst_time = instant.astimezone(KST)
    return kst_time.hour == 0 and kst_time.minute < window


def _avoid_maintenance(start: datetime, actions: list, maint: int) -> datetime:
    if maint <= 0:
        return start
    for _ in range(4):
        bad = [
            at
            for action in actions
            if action.op in MONETARY_OPS
            and in_maintenance_window(at := start + timedelta(seconds=action.offset), maint)
        ]
        if not bad:
            return start
        earliest = min(bad).astimezone(KST)
        window_end = earliest.replace(hour=0, minute=maint, second=0, microsecond=0)
        start += (window_end - earliest) + timedelta(seconds=1)
    return start


EXTRA_ROLE = {"A": "receiver", "D": "second", "E": "receiver", "purchase": "receiver", "household": "second"}


@dataclass(frozen=True)
class Schedule:
    episodes: list[Episode]
    dormant: set[str]
    profiles: dict[str, str]
    reserved: set[str]
    blocked_days: dict[date, set[str]] = field(default_factory=dict)


def _quotas(override: dict[str, int] | None, defaults: dict[str, int], multiplier: int) -> dict[str, int]:
    if override is None:
        return {kind: count * multiplier for kind, count in defaults.items()}
    if override.keys() - defaults.keys() or any(type(n) is not int or n < 0 for n in override.values()):
        raise ValueError("scenario quotas must be nonnegative integers keyed by A–G or lookalike")
    return {kind: override.get(kind, 0) for kind in defaults}


def _validate_bc(quotas: dict[str, int], window: str) -> None:
    if not quotas["C"] <= quotas["B"] <= 2 * quotas["C"]:
        raise ValueError(f"unschedulable {window} B/C quotas: require C <= B <= 2*C")


def _kinds(quotas: dict[str, int]) -> Iterator[str]:
    for kind in ("A", "D", "E", "F", "G"):
        yield from (kind for _ in range(quotas[kind]))
    for index in range(quotas["lookalike"]):
        yield LOOKALIKE_KINDS[index % len(LOOKALIKE_KINDS)]


def _required_subjects(quotas: dict[str, int]) -> int:
    cycles, remainder = divmod(quotas["lookalike"], len(LOOKALIKE_KINDS))
    paired_lookalikes = 2 * cycles + sum(kind in EXTRA_ROLE for kind in LOOKALIKE_KINDS[:remainder])
    return sum(quotas.values()) + quotas["A"] + quotas["D"] + quotas["E"] + paired_lookalikes


def build_schedule(
    config: ScheduleConfig,
    seed: int,
    first: date,
    days: int,
    live_start: datetime | None,
    customers: list[str],
) -> Schedule:
    if type(config.amplification) is not int or config.amplification <= 0:
        raise ValueError("amplification must be a positive integer")
    if type(days) is not int or days < 0:
        raise ValueError("days must be a nonnegative integer")
    if len(set(customers)) != len(customers):
        raise ValueError("customer roster contains duplicate subjects")

    annual = _quotas(config.annual, ANNUAL_RATES, config.amplification)
    live = _quotas(config.live, LIVE_RATES, 1)
    if live_start is None:
        live = dict.fromkeys(live, 0)
    _validate_bc(annual, "annual")
    _validate_bc(live, "live")

    end = first + timedelta(days=days)
    latest_annual = end - timedelta(days=1)
    live_phase = None
    if live_start is not None:
        if live_start.utcoffset() is None:
            raise ValueError("live_start must be timezone-aware")
        cutover = live_start.astimezone(KST)
        if not first <= cutover.date() <= end:
            raise ValueError("unschedulable live cutover outside the schedule date range")
        live_phase = cutover + LIVE_START_OFFSET
        if live["D"] and (live_phase + LIVE_D_MIDNIGHT_MARGIN).date() != cutover.date():
            raise ValueError("unschedulable live D near KST midnight")
        if live["D"] and (live_phase - D_PREPARATION).date() < first:
            raise ValueError("unschedulable live D: two preparation dates are required before cutover")
        full_phase = timedelta(hours=BACKFILL_HOUR) + ANNUAL_PHASE_LENGTH
        latest_annual = min(latest_annual, (cutover - full_phase).date())

    if any(annual.values()) and latest_annual < first:
        raise ValueError("unschedulable annual episodes: no eligible business date")
    if annual["D"] and latest_annual - first < D_PREPARATION:
        raise ValueError("unschedulable annual D: three business dates are required")

    needed = _required_subjects(annual) + _required_subjects(live)
    if needed > len(customers):
        raise ValueError(f"unschedulable quotas: need {needed} distinct customers, have {len(customers)}")

    rng = Random(seed)
    roster = sorted(customers)
    rng.shuffle(roster)
    names = tuple(PROFILES)
    profiles = {customer: names[i % len(names)] for i, customer in enumerate(roster)}
    victim_count = annual["B"] + live["B"]
    victims = [customer for customer in roster if profiles[customer] == "older"][:victim_count]
    if len(victims) < victim_count:
        raise ValueError("unschedulable B quota: insufficient distinct older customers")
    victim_set = set(victims)
    victim_pool = iter(victims)
    pool = (customer for customer in roster if customer not in victim_set)
    schedule = Schedule([], set(), profiles, set())

    def add(kind: str, roles: dict[str, str], window: str, carry_in: bool = False) -> None:
        if live_phase is not None and window == "live":
            start = live_phase - D_PREPARATION if carry_in else live_phase
            if kind == "F":
                start = datetime.combine(start.date(), time(3), KST)
                if start < live_phase:
                    start += timedelta(days=1)
        else:
            # Sample the completion date first so even the last D fits.
            span = (latest_annual - first).days
            earliest = D_PREPARATION.days if kind == "D" else int(kind in PRIOR_DAY)
            if earliest > span:
                raise ValueError(f"unschedulable {kind}: not enough dates for the setup and episode")
            finish = first + timedelta(days=rng.randint(earliest, span))
            start = datetime.combine(finish, time(3 if kind == "F" else BACKFILL_HOUR), KST)
            if kind == "D":
                start -= D_PREPARATION
        number = len(schedule.episodes)
        episode = Episode(f"{window}-{kind}-{number}", kind, start, roles, window, carry_in)
        actions = steps(episode)
        if window == "live":
            shifted = _avoid_maintenance(start, actions, _maintenance_minutes())
            if shifted != start:
                start = shifted
                episode = Episode(f"{window}-{kind}-{number}", kind, start, roles, window, carry_in)
        schedule.episodes.append(episode)
        schedule.reserved.update(roles.values())
        begins = (start + timedelta(seconds=actions[0].offset)).date()
        last = (start + timedelta(seconds=actions[-1].offset)).date()
        if begins < first:
            raise ValueError(f"unschedulable {kind}: setup predates backfill")
        for i in range((last - begins).days + 1):
            schedule.blocked_days.setdefault(begins + timedelta(days=i), set()).update(roles.values())

    for window, quotas in (("annual", annual), ("live", live)):
        for index in range(quotas["C"]):
            mule = next(pool)
            roles = {"victim_1": next(victim_pool), "mule": mule}
            if index < quotas["B"] - quotas["C"]:
                roles["victim_2"] = next(victim_pool)
            schedule.dormant.add(mule)
            add("BC", roles, window)

        for kind in _kinds(quotas):
            roles = {"actor": next(pool)}
            if kind in EXTRA_ROLE:
                roles[EXTRA_ROLE[kind]] = next(pool)
            add(kind, roles, window, carry_in=kind == "D" and window == "live")

    schedule.episodes.sort(key=lambda episode: (episode.start, episode.id))
    return schedule


def normal_episodes(
    schedule: Schedule,
    config: ScheduleConfig,
    seed: int,
    day: date,
    first: date,
) -> Iterator[Episode]:
    """Generate one date in time order.

    A consumer using wider time buckets must process a subject's actions in
    this order, including transfers where that subject is the receiver.
    """
    if not config.normal or day < first:
        return

    offset = (day - date(day.year, 1, 1)).days
    year_days = 365 + isleap(day.year)
    start_of_day = datetime.combine(day, time(), KST)
    excluded = schedule.dormant | schedule.blocked_days.get(day, set())
    customers = [customer for customer in schedule.profiles if customer not in excluded]
    episodes = []
    for index, customer in enumerate(customers):
        profile = PROFILES[schedule.profiles[customer]]
        phase = stable_number(seed, customer) % year_days
        # Adjacent senders in the ring use lanes three minutes apart, including the wrap.
        # The transfer follows its login by 60 seconds, so both steps fit in a lane.
        lane = 2 if index == len(customers) - 1 and index % 2 == 0 else index % 2
        for activity_index, kind in enumerate(NORMAL_SCHEDULE_KINDS):
            cadence = profile.cadence(kind)
            activity_phase = (phase + 73 * activity_index) % year_days
            if (offset * cadence.annual + activity_phase) % year_days < year_days - cadence.annual:
                continue
            roles = {"actor": customer}
            if kind == "normal_transfer":
                if len(customers) < 2:
                    continue
                roles["receiver"] = customers[(index + 1) % len(customers)]
                # Spread transfer pairs across the hour on 0-mod-3 minutes while keeping
                # adjacent ring senders 3 minutes apart so login + 60s transfer never overlap.
                bucket = stable_number(seed, day.isoformat(), str(index // 2)) % 6
                minute = bucket * 9 + 3 * lane
                if stable_number(seed, customer, day.isoformat(), "visit") % 4 != 0:
                    kind = "normal_visit"
            else:
                # Spread non-transfer activities across 1/2-mod-3 minutes so a customer's
                # activities never share a minute with each other or with normal_transfer.
                slot = stable_number(seed, customer, day.isoformat()) % 9
                minute = slot * 6 + (1, 2, 4, 5)[(activity_index - 1) % 4]
                if kind == "normal_income":
                    roles["activity"] = profile.income_activity
            start = start_of_day + timedelta(hours=cadence.hour, minutes=minute, seconds=phase % 60)
            episodes.append(Episode(f"{kind}:{day}:{customer}", kind, start, roles, "annual"))

    yield from sorted(episodes, key=lambda episode: (episode.start, episode.id))


def normal_amount(schedule: Schedule, episode: Episode, seed: int) -> int:
    """Resolve a repeatable amount in KRW using the actor's profile."""
    profile = PROFILES[schedule.profiles[episode.roles["actor"]]]
    return profile.amount(episode.kind, Random(stable_number(seed, episode.id)))


def estimate_normal_requests(
    schedule: Schedule,
    config: ScheduleConfig,
    days: int = 365,
    *,
    include_reserved: bool = True,
) -> int:
    """Estimate annual requests, including the reduction for dormant and reserved days."""
    if not config.normal:
        return 0
    customers = [
        customer for customer in schedule.profiles if include_reserved or customer not in schedule.dormant
    ]
    total = sum(PROFILES[schedule.profiles[customer]].annual_requests for customer in customers)
    if len(customers) == 1:
        profile = PROFILES[schedule.profiles[customers[0]]]
        total -= profile.transfer_request_cost
    blocked = sum(
        PROFILES[schedule.profiles[customer]].annual_requests
        for customers_on_day in schedule.blocked_days.values()
        for customer in customers_on_day - schedule.dormant
    )
    return round(total * days / 365 - (blocked / 365 if not include_reserved else 0))
