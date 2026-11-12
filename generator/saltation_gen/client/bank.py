"""Calls the bank exactly as a channel would, plus simulated context headers.

Every request carries an opaque request ID (ULID) chosen by the generator, so
ground truth can be written before the request is sent (spec F-31, F-34).
"""

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
    def __init__(self, base_url: str, timeout: float = 30.0):
        self._http = httpx.AsyncClient(base_url=base_url, timeout=timeout)

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
    ) -> tuple[int, dict[str, Any]]:
        headers = {"X-Request-Id": request_id or new_ulid()}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if ctx:
            headers.update({"X-Sim-Ip": ctx.ip, "X-Sim-Region": ctx.region, "X-Sim-Device": ctx.device})
        res = await self._http.request(method, path, headers=headers, json=json)
        body = res.json() if res.content else {}
        if res.status_code not in accept:
            raise BankError(res.status_code, body)
        return res.status_code, body

    async def seed_customers(self, customers: list[dict[str, Any]]) -> None:
        await self._call("POST", "/internal/v1/seed/customers", json={"customers": customers})

    async def seed_accounts(self, accounts: list[dict[str, Any]]) -> list[dict[str, Any]]:
        _, body = await self._call("POST", "/internal/v1/seed/accounts", json={"accounts": accounts})
        return body["accounts"]

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
    ) -> tuple[int, dict[str, Any]]:
        """Returns (status, body); 200 done, 422 rejected by the bank."""
        return await self._call(
            "POST",
            "/v2.0/transfer/withdraw/fin_num",
            request_id,
            token,
            ctx,
            json={
                "bank_tran_id": bank_tran_id(),
                "fintech_use_num": fintech_use_num,
                "tran_amt": str(amount),
                "tran_dtime": time.strftime("%Y%m%d%H%M%S"),
                "transfer_purpose": "TR",
                "wd_print_content": memo[:14],
                "recv_client_name": to_name,
                "recv_client_bank_code": "998",  # contracts/codes/institutions.json
                "recv_client_account_num": to_account,
            },
            accept=(200, 422),
        )
