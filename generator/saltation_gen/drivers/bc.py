"""A bounded synthetic B-to-C transfer and the receiving account's cash-out."""

from dataclasses import asdict, replace
from datetime import datetime, timedelta, timezone
from typing import Any

import httpx

from saltation_gen.client.bank import (
    BankClient,
    BankError,
    Context,
    atm_idempotency_key,
    bank_tran_id,
    new_ulid,
)
from saltation_gen.config import ATM_TERMINALS, BANK_CODE
from saltation_gen.personas.normal import home_context
from saltation_gen.truth import Label, TruthStore
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal

KST = timezone(timedelta(hours=9))


async def run(
    bank: BankClient,
    truth: TruthStore,
    run_id: str,
    sender_account: str,
    receiver_account: str,
    transfer_amount: int,
    withdraw_amount: int,
    *,
    owner: str | None = None,
    exited_owner: str | None = None,
) -> dict[str, Any]:
    if sender_account == receiver_account or min(transfer_amount, withdraw_amount) <= 0:
        raise RecoveryError("the proof requires distinct accounts and positive amounts")

    journal = RecoveryJournal(truth, run_id, owner or new_ulid())
    inputs = {
        "sender_account": sender_account,
        "receiver_account": receiver_account,
        "transfer_amount": transfer_amount,
        "withdraw_amount": withdraw_amount,
    }
    manifest = await journal.claim(inputs, exited_owner)
    try:
        proof = _Proof(bank, journal, manifest["operations"])
        if not manifest.get("completed"):
            if proof.state("withdraw") != "succeeded":
                roster = {customer["account_no"]: customer for customer in await truth.load_roster()}
                try:
                    sender, receiver = roster[sender_account], roster[receiver_account]
                except KeyError:
                    raise RecoveryError("both accounts must be present in the synthetic roster") from None
                if sender["customer_no"] == receiver["customer_no"]:
                    raise RecoveryError("the proof requires distinct customers")
                await proof.execute(sender, receiver, transfer_amount, withdraw_amount)
            await journal.finish()
        return {
            "run_id": run_id,
            "requests": {name: op["request_id"] for name, op in proof.operations.items()},
        }
    finally:
        await journal.release()


class _Proof:
    def __init__(self, bank: BankClient, journal: RecoveryJournal, operations: dict[str, Any]):
        self.bank = bank
        self.journal = journal
        self.operations = operations

    def state(self, name: str) -> str | None:
        return self.operations.get(name, {}).get("state")

    async def prepare(
        self,
        name: str,
        context: Context,
        payload: dict[str, Any],
        subjects: list[tuple[str, str, str]],
    ) -> None:
        if name in self.operations:
            return
        rid = new_ulid()
        run_id = self.journal.run_id
        labels = [
            asdict(Label(rid, kind, subject, role, run_id, role, f"{run_id}:{role}"))
            for kind, subject, role in subjects
        ]
        op = {
            "request_id": rid,
            "context": asdict(context),
            "payload": payload,
            "labels": labels,
            "state": "prepared",
        }
        await self.journal.prepare(name, op)
        self.operations[name] = op

    async def execute(self, sender, receiver, transfer_amount: int, withdraw_amount: int) -> None:
        if self.state("atm_start") in ("unconfirmed", "succeeded") or self.state("withdraw") == "unconfirmed":
            raise RecoveryError("ATM state requires diagnosis before this run can continue")

        if self.state("transfer") != "succeeded":
            await self.transfer(sender, receiver, transfer_amount)

        recorded_time = self.operations["transfer"]["payload"]["business_time"]
        if recorded_time != (await self.frozen_clock()).isoformat():
            raise RecoveryError("bank business time changed before ATM dispatch")

        terminal = ATM_TERMINALS[0]
        ctx = replace(home_context(receiver), region=terminal["region"])
        account = receiver["account_no"]
        subject = [("account", account, "C")]
        await self.prepare(
            "atm_start", ctx, {"terminal_id": terminal["terminal_id"], "account_no": account}, subject
        )
        await self.prepare(
            "withdraw", ctx, {"idempotency_key": atm_idempotency_key(), "amount": withdraw_amount}, subject
        )

        session = self.operations["atm_start"]
        rid, atm_ctx = await self.dispatch("atm_start")
        try:
            status, body = await self.bank.atm_start_session(
                rid, **session["payload"], pin=receiver["atm_pin"], ctx=atm_ctx
            )
            token = body.get("atm_session_token")
        except (BankError, httpx.HTTPError, ValueError):
            raise RecoveryError("ATM session outcome is unresolved") from None
        if status != 200 or body.get("rsp_code") != "A0000" or not token:
            raise RecoveryError("ATM session outcome is unresolved")
        await self.confirm("atm_start")

        if recorded_time != (await self.frozen_clock()).isoformat():
            raise RecoveryError("bank business time changed before withdrawal dispatch")
        rid, atm_ctx = await self.dispatch("withdraw")
        try:
            status, body = await self.bank.atm_withdraw(
                rid, token, **self.operations["withdraw"]["payload"], ctx=atm_ctx
            )
        except (BankError, httpx.HTTPError, ValueError):
            raise RecoveryError("ATM withdrawal outcome is unresolved") from None
        if status != 200 or body.get("rsp_code") != "A0000":
            raise RecoveryError("ATM withdrawal outcome is unresolved")
        await self.confirm("withdraw")

    async def transfer(self, sender, receiver, amount: int) -> None:
        clock = await self.frozen_clock()
        if self.state("transfer") is None:
            await self.prepare(
                "transfer",
                home_context(sender),
                {
                    "business_time": clock.isoformat(),
                    "sender_customer": sender["customer_no"],
                    "transaction_key": bank_tran_id(),
                    "fintech_use_num": sender["fintech_use_num"],
                    "to_account": receiver["account_no"],
                    "to_name": receiver["name"],
                    "amount": amount,
                    "tran_dtime": clock.strftime("%Y%m%d%H%M%S"),
                    "memo": "",
                    "bank_code": BANK_CODE,
                },
                [("account", sender["account_no"], "B"), ("account", receiver["account_no"], "C")],
            )
        payload = self.operations["transfer"]["payload"].copy()
        recorded_time = payload.pop("business_time")
        if payload.pop("sender_customer") != sender["customer_no"]:
            raise RecoveryError("the roster no longer identifies the prepared sender")
        if recorded_time != clock.isoformat():
            raise RecoveryError("bank business time differs from the prepared transfer")

        token = await self.login(sender)
        if recorded_time != (await self.frozen_clock()).isoformat():
            raise RecoveryError("bank business time changed before transfer dispatch")

        rid, ctx = await self.dispatch("transfer", can_replay=True)
        try:
            status, body = await self.bank.transfer(rid, token, ctx, **payload)
        except (BankError, httpx.HTTPError, ValueError):
            raise RecoveryError("transfer outcome is unresolved") from None
        if status != 200 or (body.get("rsp_code"), body.get("bank_rsp_code")) != ("A0000", "000"):
            raise RecoveryError("transfer outcome is unresolved")
        await self.confirm("transfer")

    async def login(self, sender) -> str:
        attempt = 1
        while self.state(f"login_{attempt}") == "succeeded":
            attempt += 1
        name = f"login_{attempt}"
        await self.prepare(
            name, home_context(sender), {"phone": sender["phone"]}, [("customer", sender["customer_no"], "B")]
        )
        rid, ctx = await self.dispatch(name)
        try:
            token = await self.bank.login(
                rid, self.operations[name]["payload"]["phone"], sender["password"], ctx
            )
        except (BankError, httpx.HTTPError, ValueError, KeyError):
            raise RecoveryError("sender login outcome is unresolved") from None
        if not token:
            raise RecoveryError("sender login outcome is unresolved")
        await self.confirm(name)
        return token

    async def dispatch(self, name: str, *, can_replay: bool = False) -> tuple[str, Context]:
        op = self.operations[name]
        state = op["state"]
        if state != "prepared" and not (can_replay and state == "unconfirmed"):
            raise RecoveryError(f"{name} outcome is unresolved")
        await self.journal.ensure_labels(op["request_id"], op["labels"])
        await self.journal.transition(name, state, "unconfirmed")
        op["state"] = "unconfirmed"
        return op["request_id"], Context(**op["context"])

    async def confirm(self, name: str) -> None:
        await self.journal.transition(name, "unconfirmed", "succeeded")
        self.operations[name]["state"] = "succeeded"

    async def frozen_clock(self) -> datetime:
        clock = await self.bank.clock()
        if clock["mode"] != "BACKFILL":
            raise RecoveryError("the proof requires the bank clock in BACKFILL")
        return datetime.fromisoformat(clock["business_time"]).astimezone(KST)
