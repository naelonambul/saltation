"""Live normal activity at a fixed rate (stage 1). Every request is labelled
`normal` for each subject before it is sent (spec F-34); the driver pauses
while the bank's outbox backlog is above the backpressure threshold (F-36a)."""

import asyncio
import random
import time
from dataclasses import dataclass, field
from typing import Any

from saltation_gen.client import BankClient, BankError, new_ulid
from saltation_gen.config import BACKPRESSURE_PAUSE, BACKPRESSURE_RESUME
from saltation_gen.personas.normal import next_step
from saltation_gen.truth import Label, TruthStore


@dataclass
class Stats:
    logins: int = 0
    transfers_done: int = 0
    transfers_rejected: int = 0
    errors: int = 0
    paused_seconds: float = 0.0
    statuses: dict[int, int] = field(default_factory=dict)


async def _backpressure(bank: BankClient, stats: Stats) -> None:
    if await bank.outbox_backlog() <= BACKPRESSURE_PAUSE:
        return
    started = time.monotonic()
    # Polls the bank's backlog over HTTP; there is no local event to wait on.
    while True:
        backlog = await bank.outbox_backlog()
        if backlog <= BACKPRESSURE_RESUME:
            break
        await asyncio.sleep(1)
    stats.paused_seconds += time.monotonic() - started


async def run(
    bank: BankClient, truth: TruthStore, run_id: str, seed: int, minutes: float, rate: float
) -> Stats:
    roster: list[dict[str, Any]] = await truth.load_roster()
    if len(roster) < 2:
        raise RuntimeError("seed at least two customers first")
    rng = random.Random(seed)
    tokens: dict[str, str] = {}
    stats = Stats()
    deadline = time.monotonic() + minutes * 60
    interval = 1.0 / rate
    i = 0
    while time.monotonic() < deadline:
        tick = time.monotonic()
        if i % 100 == 0:
            await _backpressure(bank, stats)
        step = next_step(rng, roster)
        s, r = step.sender, step.receiver
        try:
            token = tokens.get(s["customer_no"])
            if token is None:
                rid = new_ulid()
                await truth.label([Label(rid, "customer", s["customer_no"], "normal", run_id)])
                token = await bank.login(rid, s["phone"], s["password"], step.ctx)
                tokens[s["customer_no"]] = token
                stats.logins += 1
            rid = new_ulid()
            await truth.label(
                [
                    Label(rid, "account", s["account_no"], "normal", run_id),
                    Label(rid, "account", r["account_no"], "normal", run_id),
                ]
            )
            status, _ = await bank.transfer(
                rid, token, step.ctx, s["fintech_use_num"], r["account_no"], r["name"], step.amount, step.memo
            )
            stats.statuses[status] = stats.statuses.get(status, 0) + 1
            if status == 200:
                stats.transfers_done += 1
            else:
                stats.transfers_rejected += 1
        except BankError as e:
            stats.errors += 1
            stats.statuses[e.status] = stats.statuses.get(e.status, 0) + 1
            if e.status == 401:
                tokens.pop(s["customer_no"], None)
        i += 1
        await asyncio.sleep(max(0.0, interval - (time.monotonic() - tick)))
    return stats
