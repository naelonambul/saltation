"""Durable admission and clock control for isolated backfill fixtures."""

import asyncio
from datetime import datetime, timedelta, timezone
from typing import Any

from saltation_gen.client.bank import BankClient
from saltation_gen.config import BACKPRESSURE_PAUSE, BACKPRESSURE_RESUME
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal

KST = timezone(timedelta(hours=9))


class BackfillClock:
    def __init__(self, bank: BankClient, journal: RecoveryJournal, progress: dict[str, Any]):
        self.bank = bank
        self.journal = journal
        self.progress = progress
        self._confirmed: datetime | None = None

    async def save(self, **fields) -> None:
        progress = self.progress | fields
        await self.journal.save_progress(progress)
        self.progress = progress

    async def gate(self) -> None:
        if self.progress.get("observing_backlog"):
            await self.save(paused=True, observing_backlog=False)
        while True:
            paused = self.progress["paused"]
            if not paused:
                await self.save(observing_backlog=True)
            backlog = await self.bank.outbox_backlog()
            if paused and backlog < BACKPRESSURE_RESUME:
                await self.save(paused=False)
                return
            if not paused:
                paused = backlog > BACKPRESSURE_PAUSE
                await self.save(paused=paused, observing_backlog=False)
                if not paused:
                    return
            await asyncio.sleep(1)

    async def clock(self) -> datetime:
        clock = await self.bank.clock()
        if clock["mode"] != "BACKFILL":
            raise RecoveryError("fixture requires a bank in BACKFILL")
        return datetime.fromisoformat(clock["business_time"]).astimezone(KST)

    async def move_clock(self, target: datetime, *, gated: bool = False) -> None:
        current = datetime.fromisoformat(self.progress["current"])
        intended = self.progress["intended"]
        if target < current or (intended and datetime.fromisoformat(intended) != target):
            raise RecoveryError("clock checkpoint does not match the next planned action")
        if not gated:
            await self.gate()
        observed = await self.clock()
        if intended is None and observed != current:
            raise RecoveryError("bank clock changed outside the fixture")
        if observed not in {current, target}:
            raise RecoveryError("bank clock drifted or overshot the intended instant")
        if target != current:
            if intended is None:
                await self.save(intended=target.isoformat())
            if observed != target:
                state = await self.bank.clock_set(target.isoformat())
                if not _matches_backfill_clock(state, target) and await self.clock() != target:
                    raise RecoveryError("bank clock advance does not match the intended instant")
            await self.save(current=target.isoformat(), intended=None)
        self._confirmed = target

    async def advance_backfill(self, target: datetime) -> None:
        """Advance an exclusively leased BACKFILL clock without redundant GET /clock round-trips."""
        current = datetime.fromisoformat(self.progress["current"])
        intended = self.progress.get("intended")
        if target < current or (intended and datetime.fromisoformat(intended) != target):
            raise RecoveryError("clock checkpoint does not match the next planned action")
        if target == current and intended is None:
            if self._confirmed == current:
                return
            observed = await self.clock()
            if observed != current:
                raise RecoveryError("bank clock changed outside the fixture")
            self._confirmed = current
            return
        if intended is not None or self._confirmed != current:
            await self.move_clock(target, gated=True)
            return
        await self.save(intended=target.isoformat())
        state = await self.bank.clock_set(target.isoformat())
        if not _matches_backfill_clock(state, target) and await self.clock() != target:
            raise RecoveryError("bank clock advance does not match the intended instant")
        await self.save(current=target.isoformat(), intended=None)
        self._confirmed = target


def _matches_backfill_clock(state: Any, target: datetime) -> bool:
    if not isinstance(state, dict) or state.get("mode") != "BACKFILL":
        return False
    raw = state.get("business_time")
    if not isinstance(raw, str):
        return False
    try:
        return datetime.fromisoformat(raw).astimezone(KST) == target
    except ValueError:
        return False
