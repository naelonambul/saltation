"""Calls the bank exactly as a channel would, plus simulated context headers.

Every request carries an opaque request ID (ULID) chosen by the generator, so
ground truth can be written before the request is sent (spec F-31, F-34).
"""

import asyncio
import os
import secrets
import threading
import time
from dataclasses import dataclass, field
from typing import Any

import httpx

_CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_ALNUM = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
_lock = threading.Lock()
_last = (0, 0)


def new_ulid() -> str:
    """A monotonic ULID: 48-bit milliseconds then 80 random bits, Crockford base32."""
    global _last
    with _lock:
        ms = time.time_ns() // 1_000_000
        last_ms, last_rand = _last
        rand = last_rand + 1 if ms <= last_ms else int.from_bytes(os.urandom(10), "big")
        ms = max(ms, last_ms)
        _last = (ms, rand)
    value = (ms << 80) | (rand & ((1 << 80) - 1))
    return "".join(_CROCKFORD[(value >> (5 * i)) & 31] for i in reversed(range(26)))


def atm_idempotency_key() -> str:
    """Twenty printable characters from the monotonic ULID's low 100 bits."""
    return new_ulid()[-20:]


def bank_tran_id(client_code: str = "M2026SALTG") -> str:
    """KFTC bank_tran_id, AN(20): 10-character client code, `U`, 9 characters."""
    return f"{client_code[:10]:0<10}U" + "".join(secrets.choice(_ALNUM) for _ in range(9))


@dataclass
class Context:
    """Simulated request context, trusted by the bank only on the generator listener."""

    ip: str
    region: str
    device: str


@dataclass
class BankError(Exception):
    status: int
    body: dict[str, Any] = field(default_factory=dict)

    def __str__(self) -> str:
        return f"bank returned {self.status}: {self.body.get('rsp_code')} {self.body.get('rsp_message')}"


class BankClient:
    def __init__(
        self,
        base_url: str,
        timeout: float = 30.0,
        *,
        clock_timeout: float = 180.0,
        retry_503: int = 5,
        retry_backoff: float = 0.25,
        transport: httpx.AsyncBaseTransport | None = None,
    ):
        self._http = httpx.AsyncClient(base_url=base_url, timeout=timeout, transport=transport)
        self._clock_timeout = clock_timeout
        self._retry_503 = max(0, retry_503)
        self._retry_backoff = max(0.0, retry_backoff)

    async def close(self) -> None:
        await self._http.aclose()

    async def _call(
        self,
        method: str,
        path: str,
        request_id: str | None = None,
        token: str | None = None,
        ctx: Context | None = None,
        json: dict[str, Any] | None = None,
        accept: tuple[int, ...] = (200,),
        data: dict[str, str] | None = None,
        params: dict[str, str] | None = None,
        request_timeout: float | None = None,
    ) -> tuple[int, dict[str, Any]]:
        headers = {"X-Request-Id": request_id or new_ulid()}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if ctx:
            headers.update({"X-Sim-Ip": ctx.ip, "X-Sim-Region": ctx.region, "X-Sim-Device": ctx.device})
        req_kwargs: dict[str, Any] = {
            "headers": headers,
            "json": json,
            "data": data,
            "params": params,
        }
        if request_timeout is not None:
            req_kwargs["timeout"] = request_timeout
        max_attempts = 1 if (503 in accept or path.startswith("/internal/v1/seed/")) else self._retry_503 + 1
        for attempt in range(max_attempts):
            res = await self._http.request(method, path, **req_kwargs)
            if res.status_code == 503 and "retry-after" not in res.headers and attempt + 1 < max_attempts:
                await asyncio.sleep(self._retry_backoff * (2**attempt))
                continue
            body = res.json() if res.content else {}
            if res.status_code not in accept:
                raise BankError(res.status_code, body)
            return res.status_code, body
        raise AssertionError("unreachable")

    async def seed_customers(self, customers: list[dict[str, Any]]) -> None:
        await self._call("POST", "/internal/v1/seed/customers", json={"customers": customers})

    async def seed_accounts(self, accounts: list[dict[str, Any]]) -> list[dict[str, Any]]:
        _, body = await self._call("POST", "/internal/v1/seed/accounts", json={"accounts": accounts})
        return body["accounts"]

    async def seed_cards(self, items: list[dict[str, Any]]) -> list[dict[str, Any]]:
        _, body = await self._call("POST", "/internal/v1/seed/cards", json={"cards": items})
        return body["cards"]

    async def seed_analysts(self, items: list[dict[str, Any]]) -> int:
        _, body = await self._call("POST", "/internal/v1/seed/analysts", json={"analysts": items})
        return int(body["created"])

    async def outbox_backlog(self) -> int:
        _, body = await self._call("GET", "/internal/v1/outbox")
        return int(body["undelivered"])

    async def login(self, request_id: str, phone: str, password: str, ctx: Context) -> str:
        _, body = await self._call(
            "POST",
            "/v2.0/ext/auth/login",
            request_id,
            ctx=ctx,
            json={"phone": phone, "password": password, "device_id": ctx.device},
        )
        return body["access_token"]

    async def balance(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        fintech_use_num: str,
        tran_dtime: str,
        *,
        transaction_key: str | None = None,
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "GET",
            "/v2.0/account/balance/fin_num",
            request_id,
            token,
            ctx,
            params={
                "bank_tran_id": bank_tran_id() if transaction_key is None else transaction_key,
                "fintech_use_num": fintech_use_num,
                "tran_dtime": tran_dtime,
            },
            accept=(200, 404),
        )

    async def transfer(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        fintech_use_num: str,
        to_account: str,
        to_name: str,
        amount: int,
        memo: str = "",
        tran_dtime: str | None = None,
        bank_code: str = "998",
        *,
        transaction_key: str | None = None,
    ) -> tuple[int, dict[str, Any]]:
        """Returns (status, body); 200 done, 422 rejected by the bank."""
        return await self._call(
            "POST",
            "/v2.0/transfer/withdraw/fin_num",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id() if transaction_key is None else transaction_key,
                "fintech_use_num": fintech_use_num,
                "tran_amt": str(amount),
                "tran_dtime": tran_dtime or time.strftime("%Y%m%d%H%M%S"),
                "transfer_purpose": "TR",
                "wd_print_content": memo[:14],
                "recv_client_name": to_name,
                "recv_client_bank_code": bank_code,
                "recv_client_account_num": to_account,
            },
            accept=(200, 422),
        )

    async def seed_counterparties(self, items: list[dict[str, Any]]) -> int:
        _, body = await self._call("POST", "/internal/v1/seed/counterparties", json={"counterparties": items})
        return int(body["created"])

    async def seed_counterparty_clients(self, items: list[dict[str, Any]]) -> int:
        _, body = await self._call(
            "POST", "/internal/v1/seed/counterparty_clients", json={"counterparty_clients": items}
        )
        return int(body["created"])

    async def clock(self) -> dict[str, Any]:
        _, body = await self._call("GET", "/internal/v1/clock")
        return body

    async def clock_set(self, business_time: str) -> dict[str, Any]:
        _, body = await self._call(
            "POST",
            "/internal/v1/clock/set",
            json={"business_time": business_time},
            request_timeout=self._clock_timeout,
        )
        return body

    async def clock_advance(self, seconds: int) -> dict[str, Any]:
        _, body = await self._call(
            "POST",
            "/internal/v1/clock/advance",
            json={"seconds": seconds},
            request_timeout=self._clock_timeout,
        )
        return body

    async def go_live(self) -> dict[str, Any]:
        _, body = await self._call("POST", "/internal/v1/mode/live", request_timeout=self._clock_timeout)
        return body

    async def sms(
        self, phone: str, *, request_id: str | None = None, ctx: Context | None = None
    ) -> dict[str, Any] | None:
        status, body = await self._call(
            "GET", f"/internal/v1/sms/{phone}", request_id, ctx=ctx, accept=(200, 404)
        )
        return None if status == 404 else body

    async def client_token(
        self,
        client_id: str,
        client_secret: str,
        *,
        scope: str = "oob",
        request_id: str | None = None,
        ctx: Context | None = None,
    ) -> str:
        _, body = await self._call(
            "POST",
            "/oauth/2.0/token",
            request_id,
            ctx=ctx,
            data={
                "client_id": client_id,
                "client_secret": client_secret,
                "scope": scope,
                "grant_type": "client_credentials",
            },
        )
        return body["access_token"]

    async def card_authorize(
        self, request_id: str, token: str, ctx: Context, payload: dict[str, Any]
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST", "/card/v1/authorize", request_id, token, ctx, json=payload, accept=(200, 422)
        )

    async def inbound_deposit(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        cntr_account_num: str,
        fintech_use_num: str,
        amount: int,
        sender_name: str,
        tran_dtime: str,
        memo: str = "",
        *,
        transaction_key: str | None = None,
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/transfer/deposit/fin_num",
            request_id,
            token,
            ctx,
            json={
                "cntr_account_type": "N",
                "cntr_account_num": cntr_account_num,
                "wd_print_content": memo[:20],
                "name_check_option": "off",
                "tran_dtime": tran_dtime,
                "req_cnt": "1",
                "req_list": [
                    {
                        "tran_no": "1",
                        "bank_tran_id": bank_tran_id() if transaction_key is None else transaction_key,
                        "fintech_use_num": fintech_use_num,
                        "print_content": memo[:20],
                        "tran_amt": str(amount),
                        "req_client_name": sender_name[:20],
                        "transfer_purpose": "TR",
                    }
                ],
            },
            accept=(200, 422),
        )

    async def inquiry_receive(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        bank_code: str,
        account_num: str,
        amount: int,
        tran_dtime: str,
        memo: str = "",
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/inquiry/receive",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id(),
                "bank_code_std": bank_code,
                "account_num": account_num,
                "tran_amt": str(amount),
                "tran_dtime": tran_dtime,
                "print_content": memo[:20],
            },
            accept=(200, 422),
        )

    async def register_payee(
        self, request_id: str, token: str, ctx: Context, bank_code: str, account_num: str, alias: str = ""
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/ext/payees",
            request_id,
            token,
            ctx,
            json={"bank_code_std": bank_code, "account_num": account_num, "alias": alias[:40]},
            accept=(200, 409, 422),
        )

    async def delete_payee(
        self, request_id: str, token: str, ctx: Context, payee_id: str
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "DELETE",
            "/v2.0/ext/payees",
            request_id,
            token,
            ctx,
            params={"payee_id": str(payee_id)},
            accept=(200, 404),
        )

    async def password_reset_request(
        self, request_id: str, phone: str, ctx: Context
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST", "/v2.0/ext/auth/password_reset/request", request_id, ctx=ctx, json={"phone": phone}
        )

    async def password_reset_confirm(
        self, request_id: str, phone: str, code: str, new_password: str, ctx: Context
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/ext/auth/password_reset/confirm",
            request_id,
            ctx=ctx,
            json={"phone": phone, "code": code, "new_password": new_password},
            accept=(200, 401),
        )

    async def payees(self, token: str):
        _, body = await self._call("GET", "/v2.0/ext/payees", token=token)
        return body["res_list"]

    async def products(self, token: str):
        _, body = await self._call("GET", "/v2.0/ext/products", token=token)
        return body["res_list"]

    async def deposit(self, token: str, deposit_id: str):
        _, body = await self._call("GET", f"/v2.0/ext/deposits/{deposit_id}", token=token)
        return body

    async def deposit_identity(
        self, request_id: str, token: str, ctx: Context, deposit_id: str
    ) -> tuple[int, dict[str, Any]]:
        return await self._call("GET", f"/internal/v1/deposits/{deposit_id}/identity", request_id, token, ctx)

    async def loan(self, token: str, loan_id: str):
        _, body = await self._call("GET", f"/v2.0/ext/loans/{loan_id}", token=token)
        return body

    async def open_deposit(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        fintech_use_num: str,
        terms_id: str,
        amount: int,
        tran_dtime: str,
        *,
        transaction_key: str | None = None,
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/ext/deposits",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id() if transaction_key is None else transaction_key,
                "tran_dtime": tran_dtime,
                "fintech_use_num": fintech_use_num,
                "terms_id": str(terms_id),
                "tran_amt": str(amount),
            },
            accept=(200, 404, 422),
        )

    async def take_loan(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        fintech_use_num: str,
        terms_id: str,
        amount: int,
        tran_dtime: str,
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/v2.0/ext/loans",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id(),
                "tran_dtime": tran_dtime,
                "fintech_use_num": fintech_use_num,
                "terms_id": str(terms_id),
                "tran_amt": str(amount),
            },
            accept=(200, 404, 422),
        )

    async def close_deposit(
        self,
        request_id: str,
        token: str,
        ctx: Context,
        deposit_id: str,
        tran_dtime: str,
        *,
        transaction_key: str | None = None,
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            f"/v2.0/ext/deposits/{deposit_id}/close",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id() if transaction_key is None else transaction_key,
                "tran_dtime": tran_dtime,
            },
            accept=(200, 404, 422),
        )

    async def repay_loan(
        self, request_id: str, token: str, ctx: Context, loan_id: str, amount: int, tran_dtime: str
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            f"/v2.0/ext/loans/{loan_id}/repay",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id(),
                "tran_dtime": tran_dtime,
                "tran_amt": str(amount),
            },
            accept=(200, 404, 422),
        )

    async def seed_atm_terminals(self, items: list[dict[str, Any]]) -> int:
        _, body = await self._call("POST", "/internal/v1/seed/atm_terminals", json={"atm_terminals": items})
        return len(body["atm_terminals"])

    async def atm_start_session(
        self, request_id: str, terminal_id: str, account_no: str, pin: str, ctx: Context
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/atm/v1/session",
            request_id,
            ctx=ctx,
            json={"terminal_id": terminal_id, "account_no": account_no, "pin": pin},
            accept=(200, 400, 404, 422, 503),
        )

    async def atm_end_session(self, request_id: str, token: str, ctx: Context) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST", "/atm/v1/session/end", request_id, token, ctx, accept=(200, 400, 401, 503)
        )

    async def atm_withdraw(
        self, request_id: str, token: str, idempotency_key: str, amount: int, ctx: Context
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/atm/v1/cash/withdraw",
            request_id,
            token,
            ctx,
            json={"idempotency_key": idempotency_key, "amount": str(amount)},
            accept=(200, 400, 401, 409, 422, 503),
        )

    async def atm_deposit(
        self, request_id: str, token: str, idempotency_key: str, amount: int, ctx: Context
    ) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "POST",
            "/atm/v1/cash/deposit",
            request_id,
            token,
            ctx,
            json={"idempotency_key": idempotency_key, "amount": str(amount)},
            accept=(200, 400, 401, 409, 422, 503),
        )

    async def atm_balance(self, request_id: str, token: str, ctx: Context) -> tuple[int, dict[str, Any]]:
        return await self._call(
            "GET", "/atm/v1/balance", request_id, token, ctx, accept=(200, 400, 401, 422, 503)
        )
