"""Execute the short synthetic card checkpoint against a dedicated backfill bank."""

import asyncio
import hashlib
import json
from dataclasses import asdict, replace
from datetime import UTC, date, datetime
from typing import Any

import httpx

from saltation_gen.client.bank import BankClient, BankError, Context, atm_idempotency_key, new_ulid
from saltation_gen.config import ATM_TERMINALS, BACKPRESSURE_PAUSE, BACKPRESSURE_RESUME
from saltation_gen.config.card import HOLDER_ACTIVITY_WINDOW_HOURS, NORMAL_CARD_ANNUAL_CAP, card_amount_krw
from saltation_gen.drivers.card_plan import (
    KST,
    SUBJECTS,
    VERSION,
    Action,
    enforce_normal_card_cap,
    fixture_plan,
    proof_time,
)
from saltation_gen.drivers.seed import CARD_FIXTURE_START, CARD_NETWORK_ID
from saltation_gen.personas.normal import home_context
from saltation_gen.truth import Label, TruthStore
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal

NETWORK_CONTEXT = Context("192.0.2.10", "KR-11", "card-network")
ENDPOINTS = {
    "login": "/v2.0/ext/auth/login",
    "atm_start": "/atm/v1/session",
    "atm_withdraw": "/atm/v1/cash/withdraw",
    "card": "/card/v1/authorize",
}
TERMINAL_STATES = {"succeeded", "declined"}


async def run(
    bank: BankClient, truth: TruthStore, run_id: str, seed: int, day: date, *, exited_owner: str | None = None
) -> dict[str, Any]:
    plan = fixture_plan(day, seed)
    start = proof_time(day, 8, 59)
    inputs = {
        "seed": seed,
        "version": VERSION,
        "day": day.isoformat(),
        "normal_card_annual_cap": NORMAL_CARD_ANNUAL_CAP,
        "holder_activity_window_hours": HOLDER_ACTIVITY_WINDOW_HOURS,
        "plan": [asdict(action) | {"at": action.at.isoformat()} for action in plan],
    }
    journal = RecoveryJournal(truth, run_id, new_ulid(), kind="generator-cards")
    manifest = await journal.claim(inputs, exited_owner)
    try:
        if manifest.get("completed"):
            return manifest["summary"]

        roster = {customer["customer_no"]: customer for customer in await truth.load_roster()}
        try:
            people = {name: roster[f"C{CARD_FIXTURE_START + i:08d}"] for i, name in enumerate(SUBJECTS)}
        except KeyError:
            raise RecoveryError("the reserved card fixture roster is incomplete") from None

        if len({row["account_no"] for row in people.values()}) != len(people):
            raise RecoveryError("fixture accounts must be distinct")
        if any(not row.get("card_no") for row in people.values()):
            raise RecoveryError("the fixture requires a seeded card for every account")
        enforce_normal_card_cap(plan, {name: row["customer_no"] for name, row in people.items()})
        progress = manifest.get("progress", {})
        binding = _roster_binding(people)
        if progress and progress["roster_binding"] != binding:
            raise RecoveryError("the reserved synthetic card mapping changed")
        proof = _CardRun(bank, truth, journal, people, manifest["operations"], progress)
        if not progress:
            await proof.save(
                current=start.isoformat(),
                business_start=start.isoformat(),
                intended=None,
                paused=False,
                roster_binding=binding,
                seed=seed,
                started_at=datetime.now(UTC).isoformat(),
            )
        try:
            unresolved = [name for name, op in proof.operations.items() if op["state"] == "unconfirmed"]
            if unresolved:
                raise RecoveryError(f"unconfirmed operation {unresolved[0]} requires diagnosis")
            for action in plan:
                if proof.state(action.name) not in TERMINAL_STATES:
                    await proof.execute(action)
        except (RecoveryError, BankError, httpx.HTTPError):
            await proof.save(last_summary=proof.summary(plan, finished=False))
            raise
        summary = proof.summary(plan, finished=True)
        await journal.finish(summary)
        return summary
    finally:
        await journal.release()


def _roster_binding(people: dict[str, dict[str, Any]]) -> str:
    mapping = {
        name: [row["customer_no"], row["account_no"], row["card_no"], row["phone"]]
        for name, row in people.items()
    }
    return hashlib.sha256(json.dumps(mapping, sort_keys=True).encode()).hexdigest()


class _CardRun:
    def __init__(self, bank, truth, journal, people, operations, progress):
        self.bank = bank
        self.truth = truth
        self.journal = journal
        self.people = people
        self.operations = operations
        self.progress = progress
        self.atm_token: str | None = None
        self.card_token: str | None = None

    def state(self, name: str) -> str | None:
        return self.operations.get(name, {}).get("state")

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
            raise RecoveryError("card proof requires a bank in BACKFILL")
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

    async def execute(self, action: Action) -> None:
        await self.move_clock(action.at)
        if action.kind == "card" and self.card_token is None:
            await self.gate()
            credential = await self.truth.card_networks.find_one({"_id": CARD_NETWORK_ID})
            if not credential or not credential.get("client_secret"):
                raise RecoveryError("the seeded card network credential is missing")
            self.card_token = await self.bank.client_token(
                CARD_NETWORK_ID, credential["client_secret"], scope="card", ctx=NETWORK_CONTEXT
            )
        if action.kind == "atm_withdraw" and self.atm_token is None:
            # A successful ATM session token exists only in the process that obtained it.
            # Open a new labelled session on resume, preserving the original outcome.
            attempt = 1
            while self.state(f"{action.name}_refresh_{attempt}") == "succeeded":
                attempt += 1
            refresh = replace(action, name=f"{action.name}_refresh_{attempt}", kind="atm_start", payload={})
            await self.dispatch(refresh)
        await self.dispatch(action)

    async def dispatch(self, action: Action) -> None:
        row = self.people[action.subject]
        if action.name not in self.operations:
            rid = new_ulid()
            ctx = NETWORK_CONTEXT if action.kind == "card" else home_context(row)
            if action.kind == "login":
                ctx = replace(ctx, region=action.payload["region"])
            if action.kind.startswith("atm_"):
                ctx = replace(ctx, region=ATM_TERMINALS[0]["region"])

            subject_type = "customer" if action.kind == "login" else "account"
            subject_id = row["customer_no"] if subject_type == "customer" else row["account_no"]
            run_id = self.journal.run_id
            label = Label(
                rid,
                subject_type,
                subject_id,
                action.label,
                run_id,
                action.label,
                f"{run_id}:{action.subject}",
            )
            payload = dict(action.payload)
            if action.kind in {"card", "atm_withdraw"}:
                payload["idempotency_key"] = atm_idempotency_key()
            if action.kind == "atm_start":
                payload.update(terminal_id=ATM_TERMINALS[0]["terminal_id"], account_no=row["account_no"])
            if action.kind == "login":
                payload["phone"] = row["phone"]
            op = {
                "request_id": rid,
                "endpoint": ENDPOINTS[action.kind],
                "context": asdict(ctx),
                "payload": payload,
                "actor": action.subject,
                "business_time": action.at.isoformat(),
                "labels": [asdict(label)],
                "state": "prepared",
            }
            await self.journal.prepare(action.name, op)
            self.operations[action.name] = op

        op = self.operations[action.name]
        if op["state"] != "prepared":
            raise RecoveryError(f"operation {action.name} is not prepared")
        await self.gate()
        if await self.clock() != action.at:
            raise RecoveryError("bank clock changed before card fixture dispatch")
        await self.journal.ensure_labels(op["request_id"], op["labels"])
        await self.journal.transition(action.name, "prepared", "unconfirmed")
        op["state"] = "unconfirmed"
        try:
            outcome = await self.send(action, row, op)
        except (BankError, httpx.HTTPError, KeyError, TypeError, ValueError):
            raise RecoveryError(f"outcome for {action.name} is unconfirmed") from None
        await self.journal.transition(action.name, "unconfirmed", outcome)
        op["state"] = outcome
        await self.save(last_summary={"attempts": self.counts()})

    async def send(self, action, row, op) -> str:
        rid = op["request_id"]
        ctx = Context(**op["context"])
        payload = op["payload"]
        if action.kind == "login":
            if not await self.bank.login(rid, payload["phone"], row["password"], ctx):
                raise ValueError("missing login result")
        elif action.kind == "atm_start":
            status, body = await self.bank.atm_start_session(rid, **payload, pin=row["atm_pin"], ctx=ctx)
            if status != 200 or body.get("rsp_code") != "A0000" or not body.get("atm_session_token"):
                raise ValueError("ATM session result was not successful")
            self.atm_token = body["atm_session_token"]
        elif action.kind == "atm_withdraw":
            status, body = await self.bank.atm_withdraw(rid, self.atm_token, **payload, ctx=ctx)
            if status != 200 or body.get("rsp_code") != "A0000":
                raise ValueError("ATM withdrawal result was not successful")
        else:
            status, body = await self.bank.card_authorize(
                rid, self.card_token, ctx, payload | {"card_no": row["card_no"]}
            )
            krw = card_amount_krw(payload["country"], payload["currency"], int(payload["amount"]))
            expected = (
                (422, False, "A0004", "405", "insufficient_balance")
                if action.decline
                else (200, True, "A0000", "000", None)
            )
            actual = (
                status,
                body["approved"],
                body["rsp_code"],
                body["bank_rsp_code"],
                body["decline_reason"],
            )
            if body["krw_amount"] != str(krw) or actual != expected:
                raise ValueError("card decision does not match the fixture")
            return "declined" if action.decline else "succeeded"
        return "succeeded"

    def counts(self) -> dict[str, int]:
        states = [op["state"] for op in self.operations.values()]
        return {
            "attempted": sum(state != "prepared" for state in states),
            "completed": states.count("succeeded"),
            "declined": states.count("declined"),
            "unresolved": states.count("unconfirmed"),
        }

    def summary(self, plan: list[Action], *, finished: bool) -> dict[str, Any]:
        instances: dict[str, int] = {}
        for subject in SUBJECTS:
            actions = [action for action in plan if action.subject == subject]
            if actions and all(self.state(action.name) in TERMINAL_STATES for action in actions):
                label = actions[0].label
                instances[label] = instances.get(label, 0) + 1
        return {
            "run_id": self.journal.run_id,
            "fixture_version": VERSION,
            "seed": self.progress["seed"],
            "business_start": self.progress["business_start"],
            "business_end": self.progress["current"],
            "started_at": self.progress["started_at"],
            "finished_at": datetime.now(UTC).isoformat() if finished else None,
            "attempts": self.counts(),
            "instances": instances,
            "requests": {name: op["request_id"] for name, op in self.operations.items()},
        }
