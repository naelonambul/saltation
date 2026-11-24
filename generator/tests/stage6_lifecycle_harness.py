"""Drive the isolated Stage 6 lifecycle fixture; emit JSON for the Rust proof."""

import argparse
import asyncio
import json
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import replace
from datetime import UTC, datetime, time, timedelta
from time import monotonic
from unittest.mock import patch

import httpx

from saltation_gen.client.bank import BankClient, new_ulid
from saltation_gen.config import Settings
from saltation_gen.config.stage6 import ScheduleConfig
from saltation_gen.drivers import stage6, stage6_schedule
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6 import RunConfig, run_stage6
from saltation_gen.drivers.stage6_schedule import LIVE_START_OFFSET, build_schedule
from saltation_gen.drivers.stage6_seed import world_customer_numbers
from saltation_gen.drivers.stage6_steps import steps as standard_steps
from saltation_gen.drivers.stage6_types import Episode, Step
from saltation_gen.truth import TruthStore

START = 73_000_000
POPULATION = 40
DAYS = 7
LIVE_SECONDS = 150
FIRST_LATE_BC_STEP = 290
BC_DWELL_REDUCTION = 245
ANNUAL = {"A": 1, "B": 2, "C": 2, "E": 1, "F": 1, "G": 1, "lookalike": 6}
LIVE = {"B": 2, "C": 2, "D": 2}
MODE_PATH = "/internal/v1/mode/live"
CONTROL_PATHS = {
    "/internal/v1/clock",
    "/internal/v1/clock/set",
    "/internal/v1/clock/advance",
    "/internal/v1/outbox",
    "/oauth/2.0/token",  # Counterparty credentials have no customer or account subject.
}


def fixture_steps(episode: Episode) -> list[Step]:
    actions = standard_steps(episode)
    if episode.window != "live" or episode.kind != "BC":
        return actions
    # Keep the opening and all later gaps. Only shorten the idle dwell.
    paced = [
        replace(step, offset=step.offset - BC_DWELL_REDUCTION) if step.offset >= FIRST_LATE_BC_STEP else step
        for step in actions
    ]
    offsets = [step.offset for step in paced]
    if (
        not offsets
        or offsets != sorted(offsets)
        or offsets[0] < 0
        or LIVE_START_OFFSET + timedelta(seconds=offsets[-1]) >= timedelta(seconds=LIVE_SECONDS)
    ):
        raise ValueError("fixture B/C pacing exceeds the LIVE window or changes action order")
    return paced


@contextmanager
def fixture_pacing() -> Iterator[None]:
    # Both modules import the step expander directly. Restore both after the proof.
    with patch.object(stage6, "steps", fixture_steps), patch.object(stage6_schedule, "steps", fixture_steps):
        yield


class ProofTransport(httpx.AsyncHTTPTransport):
    def __init__(self, truth: TruthStore, run_id: str):
        super().__init__()
        self.truth = truth
        self.run_id = run_id
        self.operations = truth.runs.database["stage6_operations"]
        self.sent: set[str] = set()
        self.switch_ids: list[str] = []

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        path = request.url.path
        request_id = request.headers["X-Request-Id"]
        if path == MODE_PATH:
            self.switch_ids.append(request_id)
        elif path not in CONTROL_PATHS and not path.startswith("/internal/v1/seed/"):
            row = await self.operations.find_one(
                {"run_id": self.run_id, "request_id": request_id},
                {"state": 1, "labels": 1},
            )
            assert row and row["state"] == "unconfirmed", f"{request_id}: unjournaled dispatch"
            fields = ("subject_type", "subject_id", "label", "scenario", "scenario_instance_id")
            labels = [
                tuple(label[field] for field in fields)
                async for label in self.truth.labels.find({"run_id": self.run_id, "request_id": request_id})
            ]
            expected = [tuple(label[field] for field in fields) for label in row["labels"]]
            assert expected and sorted(labels) == sorted(expected), f"{request_id}: pre-dispatch labels"
            assert request_id not in self.sent, f"{request_id}: duplicate dispatch"
            self.sent.add(request_id)
        return await super().handle_async_request(request)


class LostModeAcknowledgement(httpx.ReadTimeout):
    """The harness drops one acknowledged, committed mode switch."""


class ProofBank(BankClient):
    def __init__(self, url: str, transport: ProofTransport):
        super().__init__(url, timeout=120, transport=transport)
        self.dropped_mode_ack = False
        self.committed_start = None
        self.original_business_time = None

    async def go_live(self) -> dict:
        result = await super().go_live()
        if not self.dropped_mode_ack:
            assert result["mode"] == "LIVE", "fault requires a successful switch"
            self.committed_start = result["live_started_at"]
            self.original_business_time = result["business_time"]
            self.dropped_mode_ack = True
            raise LostModeAcknowledgement("lifecycle fixture discarded mode acknowledgement")
        return result


async def run(run_id: str) -> dict:
    with fixture_pacing():
        return await run_fixture(run_id)


async def run_fixture(run_id: str) -> dict:
    settings = Settings.from_env()
    truth = TruthStore(settings.truth_uri)
    transport = ProofTransport(truth, run_id)
    bank = ProofBank(settings.bank_url, transport)
    try:
        await truth.setup()
        now_kst = datetime.now(UTC).astimezone(KST)
        next_midnight = datetime.combine(now_kst.date() + timedelta(days=1), time(), KST)
        until_midnight = (next_midnight - now_kst).total_seconds()
        if until_midnight < 240:
            wait_s = until_midnight + 2
            print(
                f"Near KST midnight ({until_midnight:.1f}s left); waiting {wait_s:.1f}s for next date",
                file=sys.stderr,
                flush=True,
            )
            await asyncio.sleep(wait_s)
        timer = monotonic()
        started = datetime.now(UTC)
        planned = (started.astimezone(KST) + timedelta(seconds=60)).replace(microsecond=0)
        end = planned + timedelta(seconds=LIVE_SECONDS)
        if started.astimezone(KST).date() != end.date():
            raise RuntimeError("fixture needs a KST cutover with its full live window before midnight")

        config = ScheduleConfig(population=POPULATION, annual=ANNUAL, live=LIVE)
        timing = RunConfig(
            first=planned.date() - timedelta(days=DAYS - 1),
            days=DAYS,
            live_at=planned,
            live_seconds=LIVE_SECONDS,
            start=START,
        )
        schedule = build_schedule(
            config,
            settings.seed,
            timing.first,
            timing.days,
            planned,
            world_customer_numbers(POPULATION, START),
        )
        stop = asyncio.Event()
        args = (bank, truth, run_id, settings.seed, config, timing)
        print("Preparing Stage 6 backfill and carried-in D operations", file=sys.stderr, flush=True)
        prepared = await run_stage6(*args, "backfill", stop)
        assert prepared["status"] == "backfill_ready"
        ready_at = monotonic()
        seconds_until_cutover = (planned - datetime.now(KST)).total_seconds()
        print(
            f"lifecycle timing: preparation_s={ready_at - timer:.3f} "
            f"ready_before_planned_cutover_s={seconds_until_cutover:.3f}",
            file=sys.stderr,
            flush=True,
        )
        if seconds_until_cutover <= 0:
            print("Preparation passed the planned cutover instant", file=sys.stderr, flush=True)
        assert datetime.now(KST) < end, "preparation exhausted the planned live window"
        before_switch = await bank.clock()
        assert before_switch["mode"] == "BACKFILL"
        assert before_switch["live_started_at"] is None

        try:
            await run_stage6(*args, "live", stop, resume=True)
        except LostModeAcknowledgement:
            print("Lost committed mode acknowledgement. Resuming the same run.", file=sys.stderr, flush=True)
        else:
            raise AssertionError("mode switch acknowledgement was never lost")

        interrupted = await truth.runs.find_one({"_id": run_id}, {"progress": 1})
        progress = interrupted["progress"]
        assert progress["backfill_finished"] and progress["live_intended"]
        assert not progress.get("live_started"), "fault must occur before the LIVE checkpoint"
        await asyncio.sleep(3)
        after_loss = await bank.clock()
        anchor = datetime.fromisoformat(bank.committed_start)
        assert anchor.utcoffset() == timedelta(hours=9)
        assert after_loss["mode"] == "LIVE" and after_loss["live_started_at"] == bank.committed_start
        first_observation = datetime.fromisoformat(bank.original_business_time)
        assert datetime.fromisoformat(after_loss["business_time"]) > first_observation + timedelta(seconds=2)

        resumed_at = monotonic()
        print(
            f"lifecycle timing: ready_to_recovery_s={resumed_at - ready_at:.3f}",
            file=sys.stderr,
            flush=True,
        )
        result = await run_stage6(*args, "live", stop, resume=True)
        print(
            f"lifecycle timing: resume_to_completion_s={monotonic() - resumed_at:.3f} "
            f"live_window_s={LIVE_SECONDS}",
            file=sys.stderr,
            flush=True,
        )
        assert result["status"] == "completed"
        assert result["credit"] == {"annual": ANNUAL, "live": LIVE}
        assert len(transport.switch_ids) == 1, "a second mode request reached the transport"
        assert datetime.fromisoformat(result["progress"]["live_started"]) == anchor
        assert datetime.fromisoformat(result["progress"]["live_end"]) == anchor + timedelta(
            seconds=LIVE_SECONDS
        )
        mode_request_id = transport.switch_ids[0]
        repeated_switch = await bank.go_live()
        assert repeated_switch["live_started_at"] == bank.committed_start
        journal_ids = {
            row["request_id"]
            async for row in transport.operations.find({"run_id": run_id}, {"request_id": 1})
        }
        assert journal_ids == transport.sent, "journal and transport request IDs differ"
        return {
            "run_id": run_id,
            "started_at": started.isoformat(),
            "finished_at": datetime.now(UTC).isoformat(),
            "credit": result["credit"],
            "d_carry_in_schedule": result["d_carry_in_schedule"],
            "mode_request_id": mode_request_id,
            "mode_repeat_request_id": transport.switch_ids[1],
            "dispatch_count": len(transport.sent),
            "profiles": schedule.profiles,
        }
    finally:
        await bank.close()
        await truth.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_id", nargs="?")
    arguments = parser.parse_args()
    print(json.dumps(asyncio.run(run(arguments.run_id or new_ulid()))))
