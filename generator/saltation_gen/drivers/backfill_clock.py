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

    async def move_clock(self, target: datetime) -> None:
        current = datetime.fromisoformat(self.progress["current"])
        intended = self.progress["intended"]
        if target < current or (intended and datetime.fromisoformat(intended) != target):
            raise RecoveryError("clock checkpoint does not match the next planned action")
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
                await self.bank.clock_set(target.isoformat())
                if await self.clock() != target:
                    raise RecoveryError("bank clock advance does not match the intended instant")
            await self.save(current=target.isoformat(), intended=None)
