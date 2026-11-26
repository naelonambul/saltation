"""Execute journaled operations through the bank's generator contracts."""

import asyncio
import re
import secrets
from dataclasses import asdict, replace
from datetime import datetime, timedelta

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
from saltation_gen.drivers.stage6_types import Episode, Step
from saltation_gen.personas.normal import home_context
from saltation_gen.truth import Label, TruthStore
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal
from saltation_gen.truth.stage6 import Stage6Journal

CARD_CLIENT_ID = "stage6-card"
OOB_CLIENT_ID = "stage6-oob"
NETWORK_CONTEXT = Context("192.0.2.10", "KR-11", "network-client")
ATM = ATM_TERMINALS[0]
TERMINAL_BY_REGION = {terminal["region"]: terminal for terminal in ATM_TERMINALS}
TERMINAL_BY_ID = {terminal["terminal_id"]: terminal for terminal in ATM_TERMINALS}
CUSTOMER_STEPS = {"login", "bad_login", "reset_request", "sms", "reset_confirm", "payee", "term_identity"}
AUTHENTICATED = {"payee", "transfer", "balance", "term_open", "term_close", "term_identity"}
ATM_CASH = {"atm_withdraw", "atm_deposit"}
TRANSFER_STEPS = {"transfer", "payee"}
CARD_CONTEXT = {"card", "inbound"}
ENDPOINTS = {
    "login": "/v2.0/ext/auth/login",
    "bad_login": "/v2.0/ext/auth/login",
    "reset_request": "/v2.0/ext/auth/password_reset/request",
    "sms": "/internal/v1/sms/{phone}",
    "reset_confirm": "/v2.0/ext/auth/password_reset/confirm",
    "payee": "/v2.0/ext/payees",
    "transfer": "/v2.0/transfer/withdraw/fin_num",
    "balance": "/v2.0/account/balance/fin_num",
    "term_identity": "/internal/v1/deposits/{product}/identity",
    "term_open": "/v2.0/ext/deposits",
    "term_close": "/v2.0/ext/deposits/{product}/close",
    "inbound": "/v2.0/transfer/deposit/fin_num",
    "atm_start": "/atm/v1/session",
    "atm_withdraw": "/atm/v1/cash/withdraw",
    "atm_deposit": "/atm/v1/cash/deposit",
    "card": "/card/v1/authorize",
}


def _validate_terminal_outcome(episode: Episode, step: Step, terminal: str) -> None:
    expect_declined = bool(step.args.get("expect_declined"))
    if expect_declined and terminal != "declined":
        raise RecoveryError(f"expected declined outcome for {episode.id}/{step.name}")
    if not expect_declined and terminal == "declined" and not episode.kind.startswith("normal_"):
        raise RecoveryError(f"required operation declined for {episode.id}/{step.name}")


class Stage6Executor:
    def __init__(
        self,
        bank: BankClient,
        truth: TruthStore,
        journal: Stage6Journal,
        people: dict[str, dict],
        profiles: dict[str, str] | None = None,
    ):
        self.bank = bank
        self.truth = truth
        self.journal = journal
        self.people = people
        self.profiles = profiles or {}
        self.truth_labels = RecoveryJournal(truth, journal.run_id, journal.owner)
        self.customer_tokens: dict[str, tuple[str, datetime, Context]] = {}
        self.atm_tokens: dict[str, tuple[str, datetime]] = {}
        self.network_tokens: dict[str, tuple[str, datetime]] = {}
        self.sms_codes: dict[str, str] = {}
        self.network_lock = asyncio.Lock()
        self._home_contexts: dict[str, Context] = {}
        self._network_credentials: dict[str, dict] = {}
        self._backfill_clock_owned: bool = False

    async def execute(self, episode: Episode, step: Step, now: datetime) -> None:
        previous = await self.journal.get_operation(episode.id, step.name)
        if previous and previous["state"] in {"succeeded", "declined"}:
            _validate_terminal_outcome(episode, step, previous["state"])
            return
        if previous and previous["state"] != "prepared":
            raise RecoveryError(f"unresolved stage 6 operation {episode.id}/{step.name}")

        product = await self._product(episode, step)
        context = self._context(step)
        if step.op in AUTHENTICATED and not self._customer_token(step.actor, now, context):
            await self._refresh(episode, step, now, "login")
        if step.op in ATM_CASH:
            now = await self._cash_time(now)
            if not self._atm_token(step.actor, now):
                await self._refresh(episode, step, now, "atm_start")
        if step.op == "reset_confirm" and step.actor not in self.sms_codes:
            row = await self.truth.roster.find_one(
                {"customer_no": step.actor, "protected": True}, {"stage6_reset": 1}
            )
            saved = row.get("stage6_reset", {}) if row else {}
            if (saved.get("run_id"), saved.get("episode_id")) != (self.journal.run_id, episode.id):
                raise RecoveryError("confirmed SMS code is missing from protected recovery state")
            self.sms_codes[step.actor] = saved["code"]
        await self._dispatch(episode, step, now, context, product)

    async def _refresh(self, episode: Episode, step: Step, now: datetime, op: str) -> None:
        number = 1
        while True:
            name = f"{step.name}__{op}_{number}"
            previous = await self.journal.get_operation(episode.id, name)
            if previous is None or previous["state"] == "prepared":
                break
            if previous["state"] != "succeeded":
                raise RecoveryError(f"unresolved recovery operation {episode.id}/{name}")
            number += 1
        recovery = replace(step, name=name, op=op, args={}, peer=None, peer_label=None)
        await self._dispatch(episode, recovery, now, self._context(recovery), None)

    async def _product(self, episode: Episode, step: Step) -> dict | None:
        if step.op not in {"term_identity", "term_close"}:
            return None
        opened = await self.journal.get_operation(episode.id, step.args["open_step"])
        if not opened or opened["state"] != "succeeded" or opened["actor"] != step.actor:
            raise RecoveryError("term identity requires a confirmed opening by the same customer")
        prior = opened["result"]
        payer = self.people[step.actor]["account_no"]
        if prior["payer_account_no"] != payer:
            raise RecoveryError("opening payer differs from the protected roster")
        opening = {
            "opening_request_id": opened["request_id"],
            "deposit_id": prior["deposit_id"],
            "account_num_masked": prior["account_num_masked"],
            "payer_account_no": payer,
        }
        if step.op == "term_identity":
            return opening
        queried = await self.journal.get_operation(episode.id, step.args["identity_step"])
        if (
            not queried
            or queried["state"] != "succeeded"
            or queried["actor"] != step.actor
            or queried.get("dependency") != opening
        ):
            raise RecoveryError("early close requires a verified identity for the matching opening")
        try:
            pair = _verified_pair(queried["result"], opening)
        except (KeyError, TypeError, ValueError):
            raise RecoveryError("verified term identity no longer matches the opening") from None
        return opening | pair | {"lookup_request_id": queried["request_id"]}

    def _context(self, step: Step) -> Context:
        if step.op in CARD_CONTEXT:
            return NETWORK_CONTEXT
        base = self._home_contexts.get(step.actor)
        if base is None:
            base = home_context(self.people[step.actor])
            self._home_contexts[step.actor] = base
        if step.op == "atm_start" or step.op in ATM_CASH:
            base = replace(base, region=self._terminal_for(step)["region"])
        return replace(base, **step.context) if step.context else base

    def _terminal_for(self, step: Step) -> dict:
        terminal_id = step.args.get("terminal_id")
        if isinstance(terminal_id, str) and terminal_id in TERMINAL_BY_ID:
            return TERMINAL_BY_ID[terminal_id]
        return ATM

    def _labels(self, episode: Episode, step: Step, rid: str, product: dict | None) -> list[dict]:
        row = self.people[step.actor]
        cls = step.label or (
            episode.kind if episode.kind in {"A", "B", "C", "D", "E", "F", "G"} else "lookalike"
        )
        if episode.kind.startswith("normal"):
            cls = "normal"
        target = "customer" if step.op in CUSTOMER_STEPS else "account"

        def label(role: str, kind: str, subject: str) -> dict:
            instance = f"{episode.id}:{role}"
            if role == "B":
                instance = f"{instance}:{step.actor}"
            return asdict(Label(rid, kind, subject, role, self.journal.run_id, episode.kind, instance))

        labels = [label(cls, target, row[f"{target}_no"])]
        if step.op == "transfer":
            labels.append(label(step.peer_label or "normal", "account", self.people[step.peer]["account_no"]))
        if step.op == "term_close" and product:
            labels.append(label(cls, "account", product["term_account_no"]))
        return labels

    async def _dispatch(
        self, episode: Episode, step: Step, now: datetime, context: Context, product: dict | None
    ) -> None:
        row = self.people[step.actor]
        record = await self.journal.get_operation(episode.id, step.name)
        if record is None:
            rid = new_ulid()
            labels = self._labels(episode, step, rid, product)
            record = await self.journal.prepare(
                episode.id,
                step.name,
                {
                    "request_id": rid,
                    "state": "prepared",
                    "episode_kind": episode.kind,
                    "scenario_class": labels[0]["label"],
                    "persona": self.profiles.get(step.actor, "unclassified"),
                    "window": episode.window,
                    "carry_in": episode.carry_in,
                    "prior_to_live": episode.carry_in and step.offset < 172_800,
                    "operation": step.op,
                    "endpoint": ENDPOINTS[step.op],
                    "actor": step.actor,
                    "peer": step.peer,
                    "context": asdict(context),
                    "args": dict(step.args),
                    "dependency": product,
                    "request_key": atm_idempotency_key()
                    if step.op in ATM_CASH | {"card"}
                    else bank_tran_id(),
                    "business_time": now.isoformat(),
                    "labels": labels,
                },
            )
        if (
            record["state"] != "prepared"
            or record["context"] != asdict(context)
            or record["args"] != step.args
            or record.get("dependency") != product
            or record["labels"] != self._labels(episode, step, record["request_id"], product)
        ):
            raise RecoveryError(f"prepared operation changed for {episode.id}/{step.name}")
        effective_time = datetime.fromisoformat(record["business_time"])
        if effective_time != now and step.op in {"transfer", "term_open", "term_close", "inbound", "balance"}:
            observed = await self.bank.clock()
            actual = datetime.fromisoformat(observed["business_time"])
            if (
                observed["mode"] != "LIVE"
                or episode.window != "live"
                or actual.date() != effective_time.date()
            ):
                raise RecoveryError(f"prepared operation business time changed for {episode.id}/{step.name}")
            stamp = actual.isoformat()
            await self.journal.retime(episode.id, step.name, record["request_id"], stamp)
            record = record | {"business_time": stamp}
        await self.truth_labels.ensure_labels(record["request_id"], record["labels"])
        if step.op in ATM_CASH:
            now = await self._cash_time(now)
            if not self._atm_token(step.actor, now, margin=2):
                await self._refresh(episode, step, now, "atm_start")
                now = await self._cash_time(now)
                if not self._atm_token(step.actor, now, margin=2):
                    raise RecoveryError("fresh ATM session expired before dispatch")
        await self.journal.transition(episode.id, step.name, "prepared", "unconfirmed")
        try:
            result = await self._send(episode, step, row, now, context, record, product)
        except (BankError, httpx.HTTPError, KeyError, TypeError, ValueError):
            raise RecoveryError(f"unconfirmed outcome for {episode.id}/{step.name}") from None
        terminal = "declined" if result["decision"] == "rejected" else "succeeded"
        await self.journal.transition(episode.id, step.name, "unconfirmed", terminal, result)
        _validate_terminal_outcome(episode, step, terminal)

    def _build_prepared_record(
        self, episode: Episode, step: Step, now: datetime, context: Context, product: dict | None
    ) -> dict:
        rid = new_ulid()
        labels = self._labels(episode, step, rid, product)
        return {
            "request_id": rid,
            "state": "prepared",
            "episode_kind": episode.kind,
            "scenario_class": labels[0]["label"],
            "persona": self.profiles.get(step.actor, "unclassified"),
            "window": episode.window,
            "carry_in": episode.carry_in,
            "prior_to_live": episode.carry_in and step.offset < 172_800,
            "operation": step.op,
            "endpoint": ENDPOINTS[step.op],
            "actor": step.actor,
            "peer": step.peer,
            "context": asdict(context),
            "args": dict(step.args),
            "dependency": product,
            "request_key": atm_idempotency_key() if step.op in ATM_CASH | {"card"} else bank_tran_id(),
            "business_time": now.isoformat(),
            "labels": labels,
        }

    async def prepare_chunk(
        self, chunk_items: list[tuple[Episode, Step, datetime]]
    ) -> dict[tuple[str, str], dict]:
        """Batch-prepare and batch-label a chunk of normal backfill operations before dispatch."""
        batch: list[tuple[str, str, dict]] = []
        for episode, step, at in chunk_items:
            context = self._context(step)
            record = self._build_prepared_record(episode, step, at, context, None)
            batch.append((episode.id, step.name, record))
        prepared = await self.journal.prepare_batch(batch)
        labels_by_rid = {
            row["request_id"]: row["labels"] for row in prepared.values() if row["state"] == "prepared"
        }
        await self.truth_labels.ensure_labels_batch(labels_by_rid)
        return prepared

    async def execute_wave(
        self,
        wave_items: list[tuple[Episode, Step, datetime]],
        prepared_map: dict[tuple[str, str], dict],
    ) -> None:
        """Fence, concurrently dispatch, and batch-complete a wave of pre-prepared normal operations."""
        to_send: list[tuple[Episode, Step, datetime, Context, dict]] = []
        for episode, step, now in wave_items:
            record = prepared_map[(episode.id, step.name)]
            if record["state"] in {"succeeded", "declined"}:
                continue
            if record["state"] != "prepared":
                raise RecoveryError(f"unresolved stage 6 operation {episode.id}/{step.name}")
            context = self._context(step)
            if (
                record["context"] != asdict(context)
                or record["args"] != step.args
                or record.get("dependency") is not None
                or record["labels"] != self._labels(episode, step, record["request_id"], None)
            ):
                raise RecoveryError(f"prepared operation changed for {episode.id}/{step.name}")
            if step.op in AUTHENTICATED and not self._customer_token(step.actor, now, context):
                await self._refresh(episode, step, now, "login")
            if step.op in ATM_CASH and not self._atm_token(step.actor, now, margin=2):
                await self._refresh(episode, step, now, "atm_start")
                if not self._atm_token(step.actor, now, margin=2):
                    raise RecoveryError("fresh ATM session expired before dispatch")
            to_send.append((episode, step, now, context, record))
        if not to_send:
            return
        await self.journal.mark_unconfirmed_wave([entry[4] for entry in to_send])
        previous_owned = self._backfill_clock_owned
        self._backfill_clock_owned = True
        try:
            raw_results = await asyncio.gather(
                *(
                    self._send(ep, step, self.people[step.actor], now, ctx, rec, None)
                    for ep, step, now, ctx, rec in to_send
                ),
                return_exceptions=True,
            )
        finally:
            self._backfill_clock_owned = previous_owned
        confirmed: list[tuple[str, str, str, dict]] = []
        first_error: BaseException | None = None
        for (ep, step, _now, _ctx, _rec), outcome in zip(to_send, raw_results, strict=True):
            if isinstance(outcome, BaseException):
                if first_error is None:
                    if isinstance(outcome, (BankError, httpx.HTTPError, KeyError, TypeError, ValueError)):
                        first_error = RecoveryError(f"unconfirmed outcome for {ep.id}/{step.name}")
                    else:
                        first_error = outcome
                continue
            terminal = "declined" if outcome["decision"] == "rejected" else "succeeded"
            confirmed.append((ep.id, step.name, terminal, outcome))
        if confirmed:
            await self.journal.complete_wave(confirmed)
        if first_error is not None:
            raise first_error

    async def _send(
        self,
        episode: Episode,
        step: Step,
        row: dict,
        now: datetime,
        ctx: Context,
        record: dict,
        product: dict | None,
    ) -> dict:
        rid = record["request_id"]
        key = record["request_key"]
        stamp = datetime.fromisoformat(record["business_time"]).strftime("%Y%m%d%H%M%S")
        args = step.args
        if step.op == "login":
            token = await self.bank.login(rid, row["phone"], row["password"], ctx)
            if not token:
                raise ValueError("empty login token")
            self.customer_tokens[step.actor] = token, now, ctx
            return {"decision": "approved"}
        if step.op == "bad_login":
            try:
                await self.bank.login(rid, row["phone"], row["password"] + "-wrong", ctx)
            except BankError as error:
                if (error.status, error.body.get("rsp_code"), error.body.get("rsp_message")) == (
                    401,
                    "O0001",
                    "전화번호 또는 비밀번호가 올바르지 않습니다",
                ):
                    return {"decision": "expected_rejection", "http_status": 401}
                raise
            raise ValueError("synthetic wrong password was accepted")
        if step.op == "sms":
            sms = await self.bank.sms(row["phone"], request_id=rid, ctx=ctx)
            if not sms or not sms.get("code"):
                raise ValueError("no simulated reset code")
            saved = {"run_id": self.journal.run_id, "episode_id": episode.id, "code": sms["code"]}
            updated = await self.truth.roster.update_one(
                {"customer_no": step.actor, "protected": True}, {"$set": {"stage6_reset": saved}}
            )
            if updated.matched_count != 1:
                raise RecoveryError("reserved SMS code could not be saved for recovery")
            self.sms_codes[step.actor] = sms["code"]
            return {"decision": "retrieved"}
        if step.op == "reset_request":
            return _accepted(await self.bank.password_reset_request(rid, row["phone"], ctx))
        if step.op == "reset_confirm":
            return await self._confirm_reset(rid, step, row, ctx)
        if step.op in ATM_CASH | {"atm_start"}:
            return await self._atm(step, row, rid, ctx, now, key)
        if step.op in CARD_CONTEXT:
            return await self._network(step, row, rid, ctx, now, key, stamp)
        token = self._customer_token(step.actor, now, ctx)
        if step.op in TRANSFER_STEPS:
            peer = self.people[step.peer]
            if step.op == "payee":
                return _accepted(
                    await self.bank.register_payee(rid, token, ctx, BANK_CODE, peer["account_no"])
                )
            return _movement(
                await self.bank.transfer(
                    rid,
                    token,
                    ctx,
                    row["fintech_use_num"],
                    peer["account_no"],
                    peer["name"],
                    args["amount"],
                    memo="결제",
                    tran_dtime=stamp,
                    transaction_key=key,
                )
            )
        if step.op == "balance":
            return _accepted(
                await self.bank.balance(rid, token, ctx, row["fintech_use_num"], stamp, transaction_key=key)
            )
        if step.op == "term_open":
            response = await self.bank.open_deposit(
                rid,
                token,
                ctx,
                row["fintech_use_num"],
                str(args["terms_id"]),
                args["amount"],
                stamp,
                transaction_key=key,
            )
            deposit_id = response[1]["deposit_id"]
            mask = response[1]["account_num_masked"]
            if not re.fullmatch(r"[0-9]{1,12}", deposit_id) or not re.fullmatch(r"[0-9]{1,13}\*{3}", mask):
                raise ValueError("invalid opening identity")
            if response[1]["linked_fintech_use_num"] != row["fintech_use_num"]:
                raise ValueError("opening was linked to a different payer")
            return _accepted(response) | {
                "deposit_id": deposit_id,
                "account_num_masked": mask,
                "payer_account_no": row["account_no"],
            }
        if step.op == "term_identity" and product:
            response = await self.bank.deposit_identity(rid, token, ctx, product["deposit_id"])
            if response[1]["request_id"] != rid:
                raise ValueError("term identity request correlation failed")
            return _accepted(response) | _verified_pair(response[1], product)
        if step.op == "term_close" and product:
            return _accepted(
                await self.bank.close_deposit(
                    rid,
                    token,
                    ctx,
                    product["deposit_id"],
                    stamp,
                    transaction_key=key,
                )
            )
        raise ValueError("unknown stage 6 operation")

    async def _confirm_reset(self, rid: str, step: Step, row: dict, ctx: Context) -> dict:
        new_password = f"pw-{secrets.token_hex(16)}"
        result = _accepted(
            await self.bank.password_reset_confirm(
                rid, row["phone"], self.sms_codes[step.actor], new_password, ctx
            )
        )
        update = await self.truth.roster.update_one(
            {"customer_no": row["customer_no"], "password": row["password"]},
            {"$set": {"password": new_password, "credential_version": rid}, "$unset": {"stage6_reset": ""}},
        )
        if update.matched_count != 1:
            raise RecoveryError("confirmed reset could not update protected roster")
        row["password"] = new_password
        self.customer_tokens.pop(step.actor, None)
        self.sms_codes.pop(step.actor, None)
        return result | {"credential_version": rid}

    async def _atm(self, step: Step, row: dict, rid: str, ctx: Context, now: datetime, key: str) -> dict:
        if step.op == "atm_start":
            terminal = self._terminal_for(step)
            result = await self.bank.atm_start_session(
                rid, terminal["terminal_id"], row["account_no"], row["atm_pin"], ctx
            )
            decision = _accepted(result)
            self.atm_tokens[step.actor] = result[1]["atm_session_token"], now
            return decision
        token = self._atm_token(step.actor, await self._cash_time(now))
        if token is None:
            raise RecoveryError("ATM session expired before HTTP; cash was not sent")
        cash = self.bank.atm_deposit if step.op == "atm_deposit" else self.bank.atm_withdraw
        return _movement(await cash(rid, token, key, step.args["amount"], ctx))

    async def _cash_time(self, expected: datetime) -> datetime:
        if self._backfill_clock_owned:
            return expected
        observed = await self.bank.clock()
        now = datetime.fromisoformat(observed["business_time"])
        if observed["mode"] == "BACKFILL" and now != expected:
            raise RecoveryError("bank clock changed before ATM dispatch")
        if observed["mode"] == "LIVE" and now.date() != expected.date():
            raise RecoveryError("ATM operation passed its business date before dispatch")
        return now

    async def _network(
        self, step: Step, row: dict, rid: str, ctx: Context, now: datetime, key: str, stamp: str
    ) -> dict:
        card = step.op == "card"
        client_id = CARD_CLIENT_ID if card else OOB_CLIENT_ID
        credential = self._network_credentials.get(client_id)
        if credential is None:
            credential = await self.truth.card_networks.find_one({"_id": client_id})
            if not credential or not credential.get("client_secret"):
                raise RecoveryError("synthetic network authorization is missing")
            self._network_credentials[client_id] = credential
        token = await self._network_token(
            client_id, credential["client_secret"], "card" if card else "oob", now
        )
        if card:
            if not row.get("card_no"):
                raise ValueError("unknown card mapping")
            payload = step.args | {
                "amount": str(step.args["amount"]),
                "idempotency_key": key,
                "card_no": row["card_no"],
            }
            response = await self.bank.card_authorize(rid, token, ctx, payload)
            if (
                response[0] == 422
                and response[1].get("approved") is False
                and response[1].get("decline_reason")
                in {
                    "blocked_card",
                    "closed_account",
                    "insufficient_balance",
                }
            ):
                return {"decision": "rejected", "http_status": 422}
            if response[1].get("approved") is not True or response[1].get("decline_reason") is not None:
                raise ValueError("card authorization was not confirmed")
            return _accepted(response)
        sender_kind = step.args["sender_kind"]
        response = await self.bank.inbound_deposit(
            rid,
            token,
            ctx,
            credential["sender_accounts"][sender_kind],
            row["fintech_use_num"],
            step.args["amount"],
            {"salary": "급여 정산", "settlement": "판매 정산", "unrelated": "거래 정산"}[sender_kind],
            stamp,
            transaction_key=key,
        )
        if response[1]["res_list"][0]["bank_rsp_code"] != "000":
            raise ValueError("inbound deposit was not approved")
        return _accepted(response)

    async def _network_token(self, client_id: str, secret: str, scope: str, now: datetime) -> str:
        async with self.network_lock:
            cached = self.network_tokens.get(client_id)
            if cached and timedelta(0) <= now - cached[1] < timedelta(days=89):
                return cached[0]
            token = await self.bank.client_token(client_id, secret, scope=scope, ctx=NETWORK_CONTEXT)
            self.network_tokens[client_id] = token, now
            return token

    def _customer_token(self, actor: str, now: datetime, context: Context) -> str | None:
        cached = self.customer_tokens.get(actor)
        if cached and timedelta(0) <= now - cached[1] < timedelta(minutes=30) and cached[2] == context:
            return cached[0]
        return None

    def _atm_token(self, actor: str, now: datetime, *, margin: int = 0) -> str | None:
        cached = self.atm_tokens.get(actor)
        if cached and timedelta(0) <= now - cached[1] < timedelta(seconds=180 - margin):
            return cached[0]
        return None


def _accepted(response: tuple[int, dict]) -> dict:
    status, body = response
    if status != 200 or body["rsp_code"] != "A0000" or body.get("bank_rsp_code", "000") != "000":
        raise ValueError("bank did not confirm the expected outcome")
    return {"decision": "approved", "http_status": 200}


def _movement(response: tuple[int, dict]) -> dict:
    status, body = response
    if status == 422 and body.get("rsp_code") == "A0004" and body.get("bank_rsp_code") in {"405", "406"}:
        return {"decision": "rejected", "http_status": status, "bank_code": body["bank_rsp_code"]}
    return _accepted(response)


def _verified_pair(body: dict, opening: dict) -> dict:
    deposit_id = body["deposit_id"]
    term = body["term_account_no"]
    linked = body["linked_account_no"]
    if (
        deposit_id != opening["deposit_id"]
        or not all(re.fullmatch(r"[0-9]{4,16}", number) for number in (term, linked))
        or linked != opening["payer_account_no"]
        or term == linked
        or term[:-3] + "***" != opening["account_num_masked"]
    ):
        raise ValueError("term identity does not match the opening")
    return {"deposit_id": deposit_id, "term_account_no": term, "linked_account_no": linked}
