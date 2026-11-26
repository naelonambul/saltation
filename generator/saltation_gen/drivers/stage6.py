"""Run a bounded Stage 6 workload against an exclusive generator bank."""

import asyncio
import hashlib
import json
import os
import sys
from collections import Counter, defaultdict
from contextlib import suppress
from dataclasses import asdict, dataclass, replace
from datetime import UTC, date, datetime, time, timedelta

from saltation_gen.client.bank import BankClient, new_ulid
from saltation_gen.config.stage6 import LOOKALIKE_KINDS, ScheduleConfig
from saltation_gen.drivers.backfill_clock import KST, BackfillClock
from saltation_gen.drivers.stage6_execute import (
    ATM,
    TERMINAL_BY_REGION,
    Stage6Executor,
)
from saltation_gen.drivers.stage6_identity import build_schedule_identity
from saltation_gen.drivers.stage6_schedule import Schedule, build_schedule, normal_amount, normal_episodes
from saltation_gen.drivers.stage6_seed import seed_world, world_customer_numbers
from saltation_gen.drivers.stage6_steps import steps
from saltation_gen.drivers.stage6_types import Episode, Step
from saltation_gen.personas.profiles import PROFILES
from saltation_gen.truth import TruthStore
from saltation_gen.truth.recovery import RecoveryError
from saltation_gen.truth.stage6 import Stage6Journal


@dataclass(frozen=True)
class RunConfig:
    first: date
    days: int = 365
    live_at: datetime | None = None
    live_seconds: int = 86_400
    workers: int = 8
    bucket_seconds: int = 60
    start: int = 0


@dataclass(frozen=True)
class Due:
    at: datetime
    episode: Episode
    step: Step
    final: bool

    @property
    def subjects(self) -> set[str]:
        return {self.step.actor, self.step.peer} - {None}


def _digest(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _index_scenarios(schedule: Schedule) -> dict[date, list[Episode]]:
    days = defaultdict(list)
    for episode in schedule.episodes:
        dates = {(episode.start + timedelta(seconds=step.offset)).date() for step in steps(episode)}
        for day in dates:
            days[day].append(episode)
    return dict(days)


def _scenario_work(day: date, episodes: list[Episode]) -> list[Due]:
    due = []
    for episode in episodes:
        actions = steps(episode)
        for index, step in enumerate(actions):
            at = episode.start + timedelta(seconds=step.offset)
            if at.date() == day:
                due.append(Due(at, episode, step, index == len(actions) - 1))
    return due


def _work_for_day(
    schedule: Schedule,
    config: ScheduleConfig,
    run: RunConfig,
    seed: int,
    day: date,
    scenarios: list[Episode],
    live: bool,
    cutover: datetime | None,
) -> list[Due]:
    due = [item for item in _scenario_work(day, scenarios) if not live or item.episode.window == "live"]
    for episode in normal_episodes(schedule, config, seed, day, run.first):
        actions = steps(episode)
        if cutover and episode.start < cutover <= episode.start + timedelta(seconds=actions[-1].offset):
            continue
        if live:
            episode = replace(episode, window="live", id=f"live:{episode.id}")
        actor = episode.roles["actor"]
        profile = PROFILES[schedule.profiles[actor]]
        device, region = profile.context(seed, actor, episode.id)
        for index, step in enumerate(actions):
            if step.op not in {"card", "inbound"}:
                context = {"device": device}
                if not step.op.startswith("atm_"):
                    context["region"] = region
                step = replace(step, context=context)
            if episode.kind == "normal_atm" and step.op.startswith("atm_"):
                terminal = TERMINAL_BY_REGION.get(region, ATM)
                step = replace(step, args=step.args | {"terminal_id": terminal["terminal_id"]})
            if "amount" in step.args:
                step = replace(step, args=step.args | {"amount": normal_amount(schedule, episode, seed)})
            due.append(
                Due(episode.start + timedelta(seconds=step.offset), episode, step, index == len(actions) - 1)
            )
    return sorted(due, key=lambda item: (item.at, item.episode.id))


def _waves(items: list[Due], workers: int):
    wave = []
    subjects = set()
    default_terminal = ATM["terminal_id"]
    for item in items:
        involved = set(item.subjects)
        if item.step.op.startswith("atm_"):
            terminal_id = item.step.args.get("terminal_id", default_terminal)
            involved.add(f"atm:{terminal_id}")
        if len(wave) == workers or subjects & involved:
            yield wave
            wave = []
            subjects = set()
        wave.append(item)
        subjects.update(involved)
    if wave:
        yield wave


def _planned_counts(schedule: Schedule, *, live: bool) -> dict[str, dict[str, int]]:
    windows = {"annual": Counter()}
    if live:
        windows["live"] = Counter()
    for episode in schedule.episodes:
        if episode.window not in windows:
            continue
        counter = windows[episode.window]
        if episode.kind == "BC":
            counter["B"] += sum(role.startswith("victim") for role in episode.roles)
            counter["C"] += 1
        else:
            counter["lookalike" if episode.kind in LOOKALIKE_KINDS else episode.kind] += 1
    return {window: dict(counts) for window, counts in windows.items() if counts}


async def _scenario_outcomes(
    schedule: Schedule,
    journal: Stage6Journal,
    people: dict[str, dict],
) -> dict[str, dict[str, dict[str, dict[str, int]]]]:
    states, credits = await journal.scenario_outcomes()
    result = {}
    for episode in schedule.episodes:
        if episode.kind == "BC":
            roles = [
                ("C", actor, people.get(actor, {}).get("account_no"))
                if role == "mule"
                else ("B", actor, actor)
                for role, actor in episode.roles.items()
            ]
        else:
            kind = "lookalike" if episode.kind in LOOKALIKE_KINDS else episode.kind
            roles = [(kind, episode.roles["actor"], episode.id)]
        for kind, actor, identity in roles:
            group = result.setdefault(episode.window, {}).setdefault(kind, {})
            counts = group.setdefault(
                schedule.profiles[actor],
                dict.fromkeys(
                    ("planned", "attempted", "completed", "rejected", "unresolved", "carried_in"), 0
                ),
            )
            counts["planned"] += 1
            active_roles = (actor,) if episode.kind == "BC" else tuple(episode.roles.values())
            observed = set().union(*(states.get((episode.id, role), set()) for role in active_roles))
            counts["attempted"] += bool(observed - {"prepared"})
            counts["completed"] += (episode.window, kind, identity) in credits
            counts["rejected"] += "declined" in observed
            counts["unresolved"] += bool(observed - {"prepared", "succeeded", "declined"})
            counts["carried_in"] += episode.carry_in and "succeeded" in observed
    return result


async def _execute_one(executor: Stage6Executor, journal: Stage6Journal, item: Due, now: datetime) -> None:
    episode = item.episode
    await executor.execute(episode, item.step, now)
    if not item.final or episode.kind.startswith("normal_"):
        return
    episode_steps = steps(episode)
    await journal.require_success(
        episode.id,
        [step.name for step in episode_steps],
        expected_declined=[step.name for step in episode_steps if step.args.get("expect_declined")],
    )
    if episode.kind == "BC":
        for role, customer in episode.roles.items():
            if role.startswith("victim"):
                await journal.credit(episode.window, "B", customer, episode.id)
        mule = executor.people[episode.roles["mule"]]["account_no"]
        await journal.credit(episode.window, "C", mule, episode.id)
    else:
        kind = "lookalike" if episode.kind in LOOKALIKE_KINDS else episode.kind
        await journal.credit(episode.window, kind, episode.id, episode.id)


async def _wait(at: datetime, now: datetime, stop: asyncio.Event) -> bool:
    if at <= now:
        return not stop.is_set()
    try:
        await asyncio.wait_for(stop.wait(), timeout=min(2, (at - now).total_seconds()))
    except TimeoutError:
        return True
    return False


async def _gate(clock: BackfillClock, stop: asyncio.Event) -> bool:
    gate = asyncio.create_task(clock.gate())
    stopping = asyncio.create_task(stop.wait())
    try:
        done, _ = await asyncio.wait({gate, stopping}, return_when=asyncio.FIRST_COMPLETED)
        if gate in done:
            await gate
        return not stop.is_set()
    finally:
        for task in (gate, stopping):
            if not task.done():
                task.cancel()
        await asyncio.gather(gate, stopping, return_exceptions=True)


def _timing(schedule: Schedule, run: RunConfig, live_at: datetime | None, backfill_end: datetime) -> None:
    if min(run.workers, run.bucket_seconds, run.live_seconds, run.days) < 1:
        raise ValueError("days, workers, bucket size and live duration must be positive")
    if run.workers > 64 or run.bucket_seconds > 3600:
        raise ValueError("at most 64 workers and 3600 seconds per bucket are supported")
    first = datetime.combine(run.first, time(), KST)
    live_end = live_at + timedelta(seconds=run.live_seconds) if live_at else None
    for episode in schedule.episodes:
        for step in steps(episode):
            at = episode.start + timedelta(seconds=step.offset)
            if first <= at < backfill_end and (
                episode.window == "annual" or episode.carry_in or step.offset < 0
            ):
                continue
            if episode.window == "live" and live_at and live_at <= at < live_end:
                continue
            raise ValueError(f"scenario {episode.kind}/{step.name} falls outside its executable phase")


def _live_anchor(observed: dict, planned: datetime) -> datetime:
    if observed.get("mode") != "LIVE":
        raise RecoveryError("bank is no longer in the recorded LIVE mode")
    try:
        started = datetime.fromisoformat(observed["live_started_at"])
        current = datetime.fromisoformat(observed["business_time"])
        if started.utcoffset() is None or current.utcoffset() is None:
            raise ValueError("clock timestamp requires an offset")
        started = started.astimezone(KST)
    except (KeyError, TypeError, ValueError):
        raise RecoveryError("bank did not report a valid committed LIVE start") from None
    if started.date() != planned.astimezone(KST).date() or started > current + timedelta(seconds=1):
        raise RecoveryError("committed LIVE start is outside the planned cutover")
    return started


def _checked_live_time(observed: dict, started: datetime) -> datetime:
    if _live_anchor(observed, started) != started:
        raise RecoveryError("bank LIVE start differs from the recorded checkpoint")
    return datetime.fromisoformat(observed["business_time"]).astimezone(KST)


async def _begin_live(clock: BackfillClock, planned: datetime, stop: asyncio.Event) -> bool:
    bank = clock.bank
    observed = await bank.clock()
    if "live_started" in clock.progress:
        try:
            persisted = datetime.fromisoformat(clock.progress["live_started"])
            if persisted.utcoffset() is None:
                raise ValueError("offset missing")
        except (TypeError, ValueError):
            raise RecoveryError("recorded LIVE start is malformed") from None
        _checked_live_time(observed, persisted)
        return True
    if observed["mode"] != "LIVE" and clock.progress.get("live_intended"):
        deadline = asyncio.get_running_loop().time() + 30
        while observed["mode"] == "BACKFILL":
            wait = deadline - asyncio.get_running_loop().time()
            if wait <= 0:
                raise RecoveryError("original mode switch remains unconfirmed; no new switch was sent")
            with suppress(TimeoutError):
                await asyncio.wait_for(stop.wait(), timeout=min(1, wait))
            if stop.is_set():
                return False
            observed = await bank.clock()
    if observed["mode"] != "LIVE":
        if observed["mode"] != "BACKFILL":
            raise RecoveryError("bank reported an unknown mode during cutover")
        while datetime.now(KST) < planned:
            if not await _wait(planned, datetime.now(KST), stop):
                return False
        if datetime.now(KST).date() != planned.astimezone(KST).date():
            raise RecoveryError("live cutover missed its planned KST business date")
        if not await _gate(clock, stop):
            return False
        await clock.save(live_intended=True)
        observed = await bank.go_live()
    if not clock.progress.get("live_intended"):
        raise RecoveryError("bank entered LIVE without a matching durable intention")
    started = _live_anchor(observed, planned)
    await clock.save(live_started=started.isoformat(), live_observed_at=datetime.now(UTC).isoformat())
    return True


_BACKFILL_CHUNK_LIMIT = 256
_NORMAL_PRUNE_KEEP_DAYS = 7


async def _advance_backfill(clock: BackfillClock, at: datetime) -> None:
    if hasattr(clock, "advance_backfill"):
        await clock.advance_backfill(at)
    else:
        await clock.move_clock(at, gated=True)


def _is_normal_wave(wave: list[Due]) -> bool:
    return all(item.episode.kind.startswith("normal_") for item in wave)


def _partition_bucket_waves(
    bucket_waves: list[tuple[int, datetime, list[Due]]], *, allow_batch: bool
) -> list[tuple[bool, list[tuple[int, datetime, list[Due]]]]]:
    """Group consecutive normal-only backfill waves into bounded chunks."""
    segments: list[tuple[bool, list[tuple[int, datetime, list[Due]]]]] = []
    chunk: list[tuple[int, datetime, list[Due]]] = []
    chunk_ops = 0
    for entry in bucket_waves:
        _index, _at, wave = entry
        if allow_batch and _is_normal_wave(wave):
            if chunk and chunk_ops + len(wave) > _BACKFILL_CHUNK_LIMIT:
                segments.append((True, chunk))
                chunk = []
                chunk_ops = 0
            chunk.append(entry)
            chunk_ops += len(wave)
        else:
            if chunk:
                segments.append((True, chunk))
                chunk = []
                chunk_ops = 0
            segments.append((False, [entry]))
    if chunk:
        segments.append((True, chunk))
    return segments


async def _due(
    items: list[Due],
    phase: str,
    clock: BackfillClock,
    run: RunConfig,
    executor: Stage6Executor,
    journal: Stage6Journal,
    stop: asyncio.Event,
    live_started: datetime | None = None,
) -> bool:
    if not items:
        return not stop.is_set()
    day = items[0].at.date()
    date_key = f"{phase}:{day.isoformat()}"
    offset = clock.progress.get("wave_offset", 0) if clock.progress.get("wave_date") == date_key else 0
    live_end = live_started + timedelta(seconds=run.live_seconds) if live_started else None
    can_batch = (
        phase == "backfill" and hasattr(executor, "prepare_chunk") and hasattr(executor, "execute_wave")
    )
    # Preserve the contracted second for ATM/session TTLs and ledger dates. A
    # bucket bounds preparation and backpressure checks; only one wave is admitted.
    buckets: dict[int, dict[datetime, list[Due]]] = {}
    for item in items:
        bucket = int(item.at.timestamp()) // run.bucket_seconds
        buckets.setdefault(bucket, {}).setdefault(item.at, []).append(item)
    index = 0
    for times in buckets.values():
        bucket_waves: list[tuple[int, datetime, list[Due]]] = []
        for at, same_time in times.items():
            for wave in _waves(same_time, run.workers):
                index += 1
                if index <= offset:
                    continue
                bucket_waves.append((index, at, wave))
        for is_batch, segment in _partition_bucket_waves(bucket_waves, allow_batch=can_batch):
            if is_batch:
                if stop.is_set():
                    return False
                if not await _gate(clock, stop):
                    return False
                chunk_items = [
                    (item.episode, item.step, at) for _wave_idx, at, wave in segment for item in wave
                ]
                prepared_map = await executor.prepare_chunk(chunk_items)
                last_done: int | None = None
                for wave_idx, at, wave in segment:
                    if stop.is_set():
                        if last_done is not None:
                            await clock.save(wave_date=date_key, wave_offset=last_done)
                        return False
                    if all(
                        prepared_map[(item.episode.id, item.step.name)]["state"] in {"succeeded", "declined"}
                        for item in wave
                    ):
                        last_done = wave_idx
                        continue
                    await _advance_backfill(clock, at)
                    await executor.execute_wave(
                        [(item.episode, item.step, at) for item in wave], prepared_map
                    )
                    last_done = wave_idx
                if last_done is not None:
                    await clock.save(wave_date=date_key, wave_offset=last_done)
                continue
            for wave_idx, at, wave in segment:
                if stop.is_set():
                    return False
                if not await _gate(clock, stop):
                    return False
                if phase == "backfill":
                    await _advance_backfill(clock, at)
                    now = at
                else:
                    skip_wave = False
                    while True:
                        observed = await clock.bank.clock()
                        now = _checked_live_time(observed, live_started)
                        if now >= live_end:
                            if all(item.episode.kind.startswith("normal_") for item in wave):
                                skip_wave = True
                                break
                            raise RecoveryError("live measurement window expired before the next operation")
                        admitted_at = max(at, live_started)
                        if admitted_at <= now:
                            if any(item.episode.kind == "D" for item in wave) and at.date() != now.date():
                                raise RecoveryError("a live D operation missed its planned KST date")
                            break
                        if not await _wait(admitted_at, now, stop):
                            return False
                    if skip_wave:
                        continue
                outcomes = await asyncio.gather(
                    *(_execute_one(executor, journal, item, now) for item in wave),
                    return_exceptions=True,
                )
                for result in outcomes:
                    if isinstance(result, BaseException):
                        raise result
                await clock.save(wave_date=date_key, wave_offset=wave_idx)
    return True


async def run_stage6(
    bank: BankClient,
    truth: TruthStore,
    run_id: str,
    seed: int,
    config: ScheduleConfig,
    run: RunConfig,
    phase: str,
    stop: asyncio.Event,
    *,
    resume: bool = False,
    exited_owner: str | None = None,
) -> dict:
    if run.live_at is not None and run.live_at.utcoffset() is None:
        raise ValueError("live cutover requires an explicit timezone")
    live_at = run.live_at.astimezone(KST) if run.live_at else None
    if phase in ("live", "backfill-live") and live_at is None:
        raise ValueError("live cutover requires --live-at")
    if phase == "live" and not resume:
        raise ValueError("run-live requires --resume with its matching backfill checkpoint")
    if exited_owner and not resume:
        raise ValueError("exited owner recovery requires --resume")

    numbers = world_customer_numbers(config.population, run.start)
    schedule = build_schedule(config, seed, run.first, run.days, live_at, numbers)
    planned = _index_scenarios(schedule)
    steps_by_start_day = defaultdict(list)
    for episode in schedule.episodes:
        steps_by_start_day[episode.start.date()].extend(
            (episode, step, episode.start + timedelta(seconds=step.offset)) for step in steps(episode)
        )
    first_at = datetime.combine(run.first, time(), KST)
    backfill_end = (
        min(live_at, first_at + timedelta(days=run.days)) if live_at else first_at + timedelta(days=run.days)
    )
    _timing(schedule, run, live_at, backfill_end)
    preparation_at = first_at - timedelta(days=366 if schedule.dormant else 0)
    inputs = {
        "version": 2,
        "seed": seed,
        "config": asdict(config),
        "run": asdict(run)
        | {"first": run.first.isoformat(), "live_at": live_at.isoformat() if live_at else None},
        "roster_slots": _digest(numbers),
        "preparation_start": preparation_at.isoformat(),
        "backfill_start": first_at.isoformat(),
        "backfill_end": backfill_end.isoformat(),
        **build_schedule_identity(schedule, steps_by_start_day),
    }
    revision = os.environ.get("SALTATION_CODE_REVISION", "").strip()
    provenance = {"code_revision": revision} if revision else None
    journal = Stage6Journal(truth, run_id, new_ulid())
    doc = await journal.claim(inputs, exited_owner, provenance=provenance)
    clock = BackfillClock(bank, journal, doc.get("progress", {}))
    people = {}
    try:
        if doc.get("completed"):
            return doc["summary"]
        if clock.progress and not resume:
            raise RecoveryError("run already exists, use --resume")
        if await journal.unresolved():
            raise RecoveryError("run has unconfirmed operations and needs reconciliation")
        seeded = clock.progress.get("seed_state")
        if seeded == "unconfirmed":
            raise RecoveryError("world seeding was interrupted and requires inspection")
        if phase == "live" and not clock.progress.get("backfill_finished"):
            raise RecoveryError("live requires completed preparation from its matching backfill")
        if seeded is None:
            observed = await bank.clock()
            if observed["mode"] != "BACKFILL":
                raise RecoveryError("new workload requires a dedicated BACKFILL bank")
            await clock.save(seed_state="unconfirmed")
            await seed_world(
                bank,
                truth,
                seed,
                config.population,
                run.start,
                schedule.dormant,
                preparation_at,
                first_at,
                run_id=run_id,
            )
            if await clock.clock() != first_at:
                raise RecoveryError("seeded bank clock does not match the first workload date")

        people = {
            row["customer_no"]: row
            async for row in truth.roster.find({"customer_no": {"$in": numbers}, "protected": True})
        }
        if people.keys() != set(numbers):
            raise RecoveryError("protected roster does not match planned customer slots")
        roster_digest = _digest(
            [
                [
                    key,
                    *[people[key][field] for field in ("account_no", "phone", "fintech_use_num", "card_no")],
                ]
                for key in numbers
            ]
        )
        if seeded is None:
            await clock.save(
                seed_state="seeded",
                roster_digest=roster_digest,
                current=first_at.isoformat(),
                intended=None,
                paused=False,
                backfill_day=run.first.isoformat(),
            )
        elif clock.progress["roster_digest"] != roster_digest:
            raise RecoveryError("protected roster identities changed since provisioning")

        await clock.save(status="running")
        executor = Stage6Executor(bank, truth, journal, people, schedule.profiles)
        status = await _run_phases(
            planned, schedule, config, run, seed, phase, live_at, backfill_end, clock, executor, journal, stop
        )
        totals = await journal.totals()
        planned_counts = _planned_counts(schedule, live=live_at is not None)
        if status == "completed" and totals["credit"] != planned_counts:
            raise RecoveryError("credited scenario counts differ from the scheduled quotas")
        carry_in = {}
        for episode in schedule.episodes:
            if episode.carry_in:
                carry_in[episode.id] = sorted(
                    {
                        (episode.start + timedelta(seconds=step.offset)).isoformat()
                        for step in steps(episode)
                        if step.op == "atm_deposit"
                    }
                )
        await clock.save(status=status)
        summary = {
            "run_id": run_id,
            "status": status,
            "planned_scenarios": dict(Counter(ep.kind for ep in schedule.episodes)),
            "scenario_quotas": planned_counts,
            "d_carry_in_schedule": carry_in,
            "operations_by_class_and_persona": await journal.operation_breakdown(),
            "scenarios_by_window_class_and_persona": await _scenario_outcomes(schedule, journal, people),
            "progress": clock.progress,
            **totals,
        }
        if status == "completed":
            await journal.finish(summary)
        return summary
    except Exception as error:
        with suppress(Exception):
            status = "blocked" if isinstance(error, RecoveryError) else "failed"
            await clock.save(
                status=status,
                operation_counts=await journal.totals(),
                scenario_counts=await _scenario_outcomes(schedule, journal, people),
            )
        raise
    finally:
        if sys.exception() is None:
            await journal.release()
        else:
            with suppress(Exception):
                await journal.release()


async def _drain(clock: BackfillClock, stop: asyncio.Event) -> bool:
    started = asyncio.get_running_loop().time()
    while not stop.is_set():
        if await clock.bank.outbox_backlog() == 0:
            return True
        if asyncio.get_running_loop().time() - started >= 90:
            raise RecoveryError("outbox did not drain within 90 seconds")
        with suppress(TimeoutError):
            await asyncio.wait_for(stop.wait(), 1)
    return False


async def _run_phases(
    planned,
    schedule,
    config,
    run,
    seed,
    phase,
    live_at,
    backfill_end,
    clock,
    executor,
    journal,
    stop,
) -> str:
    if not clock.progress.get("backfill_finished"):
        day = date.fromisoformat(clock.progress["backfill_day"])
        while datetime.combine(day, time(), KST) < backfill_end:
            due = [
                item
                for item in _work_for_day(
                    schedule, config, run, seed, day, planned.get(day, []), False, live_at
                )
                if item.at < backfill_end
            ]
            if not await _due(due, "backfill", clock, run, executor, journal, stop):
                return "stopped"
            boundary = min(datetime.combine(day + timedelta(days=1), time(), KST), backfill_end)
            if not await _gate(clock, stop):
                return "stopped"
            await clock.move_clock(boundary, gated=True)
            completed_day = day
            day += timedelta(days=1)
            await clock.save(backfill_day=day.isoformat())
            prune_day = completed_day - timedelta(days=_NORMAL_PRUNE_KEEP_DAYS)
            if prune_day >= run.first:
                await journal.prune_completed_normal_day(prune_day.isoformat())
        await clock.save(backfill_finished=True)

    if phase == "backfill":
        if not await _drain(clock, stop):
            return "stopped"
        return "backfill_ready" if live_at else "completed"
    if not await _begin_live(clock, live_at, stop):
        return "stopped"
    started = datetime.fromisoformat(clock.progress["live_started"])
    live_end = started + timedelta(seconds=run.live_seconds)
    await clock.save(live_end=live_end.isoformat())
    planned_end = live_at + timedelta(seconds=run.live_seconds)
    day = date.fromisoformat(clock.progress.get("live_day", live_at.date().isoformat()))
    while datetime.combine(day, time(), KST) < max(planned_end, live_end):
        due = [
            item
            for item in _work_for_day(schedule, config, run, seed, day, planned.get(day, []), True, live_at)
            if live_at <= item.at and (not item.episode.kind.startswith("normal_") or item.at < live_end)
        ]
        if not await _due(due, "live", clock, run, executor, journal, stop, started):
            return "stopped"
        day += timedelta(days=1)
        await clock.save(live_day=day.isoformat())
    while True:
        observed = await clock.bank.clock()
        now = _checked_live_time(observed, started)
        if now >= live_end:
            break
        if not await _wait(live_end, now, stop):
            return "stopped"
    return "completed" if await _drain(clock, stop) else "stopped"
