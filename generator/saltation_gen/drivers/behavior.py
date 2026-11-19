"""Execute the synthetic E/F and benign behavior proof."""

import hashlib
import json
from dataclasses import asdict, replace
from datetime import UTC, date, datetime

import httpx

from saltation_gen.client.bank import BankClient, BankError, Context, bank_tran_id, new_ulid
from saltation_gen.config import BANK_CODE
from saltation_gen.drivers.backfill_clock import BackfillClock
from saltation_gen.drivers.behavior_plan import (
    F_ACTIVE_HOURS,
    FIXTURE_START,
    LABELS,
    SUBJECTS,
    VERSION,
    Action,
    fixture_plan,
    validate_plan,
)
from saltation_gen.drivers.behavior_plan import instant as fixture_instant
from saltation_gen.drivers.seed import make_customers
from saltation_gen.truth import Label, TruthStore
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal

LOGIN = "/v2.0/ext/auth/login"
TRANSFER = "/v2.0/transfer/withdraw/fin_num"


def customer_number(role: str) -> str:
    return f"C{FIXTURE_START + SUBJECTS.index(role):08d}"


async def seed_fixture(bank: BankClient, truth: TruthStore, seed: int) -> None:
    customers = make_customers(seed, len(SUBJECTS), FIXTURE_START)
    await bank.seed_customers(
        [
            {k: customer[k] for k in ("customer_no", "name", "phone", "address", "customer_type", "password")}
            for customer in customers
        ]
    )
    accounts = await bank.seed_accounts(
        [
            {k: customer[k] for k in ("customer_no", "account_no", "atm_pin")}
            | {"opening_balance": 1_000_000}
            for customer in customers
        ]
    )
    fins = {account["account_no"]: account["fintech_use_num"] for account in accounts}
    expected = {customer["account_no"] for customer in customers}
    if len(accounts) != len(expected) or fins.keys() != expected or len(set(fins.values())) != len(expected):
        raise RecoveryError("incomplete or duplicate fixture account mapping")
    await truth.roster.delete_many(
        {"customer_no": {"$in": [customer["customer_no"] for customer in customers]}}
    )
    await truth.add_roster(
        [
            customer | {"fintech_use_num": fins[customer["account_no"]], "opening_balance": 1_000_000}
            for customer in customers
        ]
    )


def context(action: Action) -> Context:
    device = 101 + SUBJECTS.index(action.actor)
    ctx = Context(f"192.0.2.{device - 90}", "KR-11", f"device-{device}")
    if action.motif == "travel":
        return replace(ctx, ip="198.51.100.17", region="US")
    if action.motif == "new_phone":
        return replace(ctx, device="device-114")
    if action.motif == "household":
        return replace(ctx, ip="192.0.2.55")
    return ctx


async def run(
    bank: BankClient, truth: TruthStore, run_id: str, seed: int, day: date, *, exited_owner: str | None = None
) -> dict:
    actions, reservations = fixture_plan(day)
    validate_plan(actions, reservations)
    first = fixture_instant(day, 9, 58).isoformat()
    inputs = {
        "version": VERSION,
        "seed": seed,
        "date": day.isoformat(),
        "f_active_hours_kst": list(F_ACTIVE_HOURS),
        "plan": [
            asdict(action) | {"at": action.at.isoformat(), "context": asdict(context(action))}
            for action in actions
        ],
        "reservations": [
            asdict(item) | {"start": item.start.isoformat(), "end": item.end.isoformat()}
            for item in reservations
        ],
    }
    journal = RecoveryJournal(truth, run_id, new_ulid(), kind="generator-behavior")
    persisted = await journal.claim(inputs, exited_owner)
    try:
        if persisted.get("completed"):
            return persisted["summary"]
        roster = {row["customer_no"]: row for row in await truth.load_roster()}
        try:
            people = {role: roster[customer_number(role)] for role in SUBJECTS}
        except KeyError:
            raise RecoveryError("behavior fixture roster is incomplete") from None
        if len({row["account_no"] for row in people.values()}) != len(people):
            raise RecoveryError("fixture accounts must be distinct")
        mapping = {
            role: [row[key] for key in ("customer_no", "account_no", "phone", "fintech_use_num", "name")]
            for role, row in people.items()
        }
        binding = hashlib.sha256(json.dumps(mapping, sort_keys=True).encode()).hexdigest()
        progress = persisted.get("progress", {})
        if progress and progress["roster_binding"] != binding:
            raise RecoveryError("behavior fixture roster changed")
        proof = _BehaviorRun(bank, journal, progress, people, persisted["operations"])
        if not progress:
            await proof.save(
                current=first,
                intended=None,
                paused=False,
                business_start=first,
                roster_binding=binding,
                started_at=datetime.now(UTC).isoformat(),
            )
        try:
            if any(op["state"] == "unconfirmed" for op in proof.operations.values()):
                raise RecoveryError("unconfirmed behavior operation requires diagnosis")
            for action in actions:
                if proof.state(action.name) != "succeeded":
                    await proof.execute(action)
        except (RecoveryError, BankError, httpx.HTTPError):
            await proof.save(last_summary=proof.summary(actions, seed, finished=False))
            raise
        result = proof.summary(actions, seed, finished=True)
        await journal.finish(result)
        return result
    finally:
        await journal.release()


class _BehaviorRun(BackfillClock):
    def __init__(self, bank, journal, progress, people, operations):
        super().__init__(bank, journal, progress)
        self.people = people
        self.operations = operations
        self.tokens: dict[str, str] = {}

    def state(self, name: str) -> str | None:
        return self.operations.get(name, {}).get("state")

    async def execute(self, action: Action) -> None:
        await self.move_clock(action.at)
        await self.prepare(action)
        if action.amount is not None and action.actor not in self.tokens:
            number = 1
            while self.state(f"{action.name}_recovery_{number}") == "succeeded":
                number += 1
            recovery = replace(action, name=f"{action.name}_recovery_{number}", amount=None)
            await self.prepare(recovery)
            await self.dispatch(recovery)
        await self.dispatch(action)

    async def prepare(self, action: Action) -> None:
        if action.name in self.operations:
            return
        rid = new_ulid()
        sender = self.people[action.actor]
        typ = "customer" if action.amount is None else "account"
        label = LABELS.get(action.motif, "lookalike")
        scope = action.motif if label != "normal" else f"normal:{action.actor}"
        run_id = self.journal.run_id
        labels = [Label(rid, typ, sender[f"{typ}_no"], label, run_id, action.motif, f"{run_id}:{scope}")]
        payload = {"phone": sender["phone"]}
        endpoint = LOGIN
        if action.amount is not None:
            receiver = self.people["receiver"]
            labels.append(
                Label(
                    rid,
                    "account",
                    receiver["account_no"],
                    "normal",
                    run_id,
                    "background",
                    f"{run_id}:normal:receiver",
                )
            )
            payload = {
                "transaction_key": bank_tran_id(),
                "fintech_use_num": sender["fintech_use_num"],
                "to_account": receiver["account_no"],
                "to_name": receiver["name"],
                "amount": action.amount,
                "tran_dtime": action.at.strftime("%Y%m%d%H%M%S"),
                "memo": "결제",
                "bank_code": BANK_CODE,
            }
            endpoint = TRANSFER
        op = {
            "state": "prepared",
            "actor": action.actor,
            "business_time": action.at.isoformat(),
            "request_id": rid,
            "endpoint": endpoint,
            "context": asdict(context(action)),
            "payload": payload,
            "labels": [asdict(item) for item in labels],
        }
        await self.journal.prepare(action.name, op)
        self.operations[action.name] = op

    async def dispatch(self, action: Action) -> None:
        op = self.operations[action.name]
        if op["state"] != "prepared":
            raise RecoveryError(f"behavior operation {action.name} is not prepared")
        rid, ctx = op["request_id"], Context(**op["context"])
        await self.gate()
        if await self.clock() != action.at:
            raise RecoveryError("bank clock changed before behavior fixture dispatch")
        await self.journal.ensure_labels(rid, op["labels"])
        await self.journal.transition(action.name, "prepared", "unconfirmed")
        op["state"] = "unconfirmed"
        try:
            if op["endpoint"] == LOGIN:
                token = await self.bank.login(
                    rid, op["payload"]["phone"], self.people[action.actor]["password"], ctx
                )
                if not token:
                    raise ValueError("missing access token")
                self.tokens[action.actor] = token
            else:
                status, body = await self.bank.transfer(rid, self.tokens[action.actor], ctx, **op["payload"])
                if status != 200 or (body["rsp_code"], body["bank_rsp_code"]) != ("A0000", "000"):
                    raise ValueError("transfer was not approved")
        except (BankError, httpx.HTTPError, KeyError, TypeError, ValueError):
            raise RecoveryError(f"outcome for {action.name} is unconfirmed") from None
        await self.journal.transition(action.name, "unconfirmed", "succeeded")
        op["state"] = "succeeded"

    def summary(self, actions, seed: int, *, finished: bool) -> dict:
        counts = {}
        for motif in {action.motif for action in actions} - {"background"}:
            group = [action for action in actions if action.motif == motif]
            if all(self.state(action.name) == "succeeded" for action in group):
                label = LABELS.get(motif, "lookalike")
                counts[label] = counts.get(label, 0) + 1
        operations = self.operations.values()
        scheduled = {action.name for action in actions}
        return {
            "run_id": self.journal.run_id,
            "seed": seed,
            "fixture_version": VERSION,
            "business_start": self.progress["business_start"],
            "business_end": self.progress["current"],
            "started_at": self.progress["started_at"],
            "finished_at": datetime.now(UTC).isoformat() if finished else None,
            "attempted": sum(op["state"] != "prepared" for op in operations),
            "completed": sum(op["state"] == "succeeded" for op in operations),
            "unresolved": sum(op["state"] == "unconfirmed" for op in operations),
            "recovery_logins": sum(
                name not in scheduled and op["state"] != "prepared" for name, op in self.operations.items()
            ),
            "instances": counts,
            "requests": {name: op["request_id"] for name, op in self.operations.items()},
        }
