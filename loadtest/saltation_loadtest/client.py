"""Async HTTP client for Saltation load tests (generator + metrics listeners)."""

from __future__ import annotations

import os
import secrets
import threading
import time
from dataclasses import dataclass
from typing import Any

import httpx

_CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_ALNUM = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
_lock = threading.Lock()
_last = (0, 0)


def new_ulid() -> str:
    global _last
    with _lock:
        ms = time.time_ns() // 1_000_000
        last_ms, last_rand = _last
        rand = last_rand + 1 if ms <= last_ms else int.from_bytes(os.urandom(10), "big")
        ms = max(ms, last_ms)
        _last = (ms, rand)
    value = (ms << 80) | (rand & ((1 << 80) - 1))
    return "".join(_CROCKFORD[(value >> (5 * i)) & 31] for i in reversed(range(26)))


def bank_tran_id(client_code: str = "M2026LTSTG") -> str:
    return f"{client_code[:10]:0<10}U" + "".join(secrets.choice(_ALNUM) for _ in range(9))


@dataclass
class Actor:
    customer_no: str
    name: str
    phone: str
    password: str
    account_no: str
    fintech_use_num: str
    token: str
    ip: str
    region: str
    device: str


@dataclass(frozen=True)
class TransferAttempt:
    request_id: str
    bank_tran_id: str
    sender: str
    receiver: str
    amount: int
    status: int
    succeeded: bool
    business_rejection: str | None
    elapsed_ms: float


class LoadClient:
    def __init__(
        self,
        bank_url: str,
        metrics_url: str,
        *,
        timeout: float = 15.0,
        max_connections: int = 256,
        bank_transport: httpx.AsyncBaseTransport | None = None,
        metrics_transport: httpx.AsyncBaseTransport | None = None,
    ):
        limits = httpx.Limits(max_connections=max_connections, max_keepalive_connections=max_connections)
        self._bank = httpx.AsyncClient(
            base_url=bank_url, timeout=timeout, limits=limits, transport=bank_transport, trust_env=False
        )
        self._metrics = httpx.AsyncClient(
            base_url=metrics_url, timeout=timeout, transport=metrics_transport, trust_env=False
        )

    async def close(self) -> None:
        await self._bank.aclose()
        await self._metrics.aclose()

    async def healthz(self) -> bool:
        res = await self._bank.get("/healthz")
        return res.status_code == 200

    async def scrape_metrics(self) -> str:
        res = await self._metrics.get("/metrics")
        res.raise_for_status()
        return res.text

    async def seed_customers(self, customers: list[dict[str, Any]]) -> None:
        res = await self._bank.post(
            "/internal/v1/seed/customers",
            headers={"X-Request-Id": new_ulid()},
            json={"customers": customers},
        )
        res.raise_for_status()

    async def seed_accounts(self, accounts: list[dict[str, Any]]) -> list[dict[str, Any]]:
        res = await self._bank.post(
            "/internal/v1/seed/accounts",
            headers={"X-Request-Id": new_ulid()},
            json={"accounts": accounts},
        )
        res.raise_for_status()
        return res.json()["accounts"]

    async def login(self, phone: str, password: str, *, ip: str, region: str, device: str) -> str:
        res = await self._bank.post(
            "/v2.0/ext/auth/login",
            headers={
                "X-Request-Id": new_ulid(),
                "X-Sim-Ip": ip,
                "X-Sim-Region": region,
                "X-Sim-Device": device,
            },
            json={"phone": phone, "password": password, "device_id": device},
        )
        res.raise_for_status()
        return res.json()["access_token"]

    async def transfer(self, sender: Actor, receiver: Actor, amount: int) -> TransferAttempt:
        """Record the outcome and the identifiers needed for independent reconciliation."""
        started = time.perf_counter()
        request_id = new_ulid()
        transaction_id = bank_tran_id()
        code = bank_code = None
        try:
            res = await self._bank.post(
                "/v2.0/transfer/withdraw/fin_num",
                headers={
                    "X-Request-Id": request_id,
                    "Authorization": f"Bearer {sender.token}",
                    "X-Sim-Ip": sender.ip,
                    "X-Sim-Region": sender.region,
                    "X-Sim-Device": sender.device,
                },
                json={
                    "bank_tran_id": transaction_id,
                    "fintech_use_num": sender.fintech_use_num,
                    "tran_amt": str(amount),
                    "tran_dtime": time.strftime("%Y%m%d%H%M%S"),
                    "transfer_purpose": "TR",
                    "wd_print_content": "LOADTEST",
                    "recv_client_name": receiver.name,
                    "recv_client_bank_code": "998",
                    "recv_client_account_num": receiver.account_no,
                },
            )
            status = res.status_code
            try:
                body = res.json()
            except ValueError:
                body = None
            if isinstance(body, dict):
                code = body.get("rsp_code")
                bank_code = body.get("bank_rsp_code")
        except httpx.HTTPError:
            status = 0
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        rejection = None
        if status == 422 and code == "A0004" and bank_code in ("403", "404", "405", "406"):
            rejection = bank_code
        return TransferAttempt(
            request_id,
            transaction_id,
            sender.account_no,
            receiver.account_no,
            amount,
            status,
            status == 200 and code == "A0000",
            rejection,
            elapsed_ms,
        )
