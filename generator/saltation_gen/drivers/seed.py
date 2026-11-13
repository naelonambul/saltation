"""Seeds customers and funded demand accounts through the internal interface
(spec F-29) and records them in the generator's roster."""

import hashlib
import random
from typing import Any

from saltation_gen.client import BankClient
from saltation_gen.truth import TruthStore

FAMILY = ["김", "이", "박", "최", "정", "강", "조", "윤", "장", "임"]
GIVEN = ["민준", "서연", "도윤", "하은", "지호", "수아", "예준", "지우", "현우", "서윤"]
BATCH = 200


def make_customers(seed: int, count: int, start: int = 0) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    out = []
    for i in range(start, start + count):
        n = f"{i:08d}"
        out.append(
            {
                "customer_no": f"C{n}",
                "name": rng.choice(FAMILY) + rng.choice(GIVEN),
                "phone": f"010{n}",
                "address": "서울",
                "customer_type": "INDIVIDUAL",
                "password": f"pw-{rng.getrandbits(48):012x}",
                "account_no": f"998{n}",
                "opening_balance": rng.choice([300_000, 1_000_000, 3_000_000, 10_000_000]),
            }
        )
    return out


async def seed(bank: BankClient, truth: TruthStore, seed: int, count: int, start: int = 0) -> int:
    customers = make_customers(seed, count, start)
    for i in range(0, len(customers), BATCH):
        batch = customers[i : i + BATCH]
        await bank.seed_customers(
            [
                {k: c[k] for k in ("customer_no", "name", "phone", "address", "customer_type", "password")}
                for c in batch
            ]
        )
        accounts = await bank.seed_accounts(
            [
                {
                    "customer_no": c["customer_no"],
                    "account_no": c["account_no"],
                    "opening_balance": c["opening_balance"],
                }
                for c in batch
            ]
        )
        fins = {a["account_no"]: a["fintech_use_num"] for a in accounts}
        await truth.add_roster([{**c, "fintech_use_num": fins[c["account_no"]]} for c in batch])
    return len(customers)


def make_counterparties(seed: int, count: int) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    return [
        {
            "counterparty_type": ("EMPLOYER", "MERCHANT", "INDIVIDUAL")[i % 3],
            "institution_code": str(901 + i % 3),
            "account_no": f"{901 + i % 3}{i:013d}",
            "name": rng.choice(FAMILY) + rng.choice(GIVEN),
        }
        for i in range(count)
    ]


def make_clients(seed: int) -> list[dict[str, Any]]:
    return [
        {
            "client_id": f"cp-{code}",
            "client_secret": hashlib.sha256(f"{seed}:cp-{code}".encode()).hexdigest(),
            "institution_code": str(code),
            "scope": "oob",
        }
        for code in range(901, 904)
    ]


async def seed_parties(bank: BankClient, seed: int, count: int) -> dict[str, int]:
    counts = {"counterparties": 0, "counterparty_clients": 0}
    for key, items, send in (
        ("counterparties", make_counterparties(seed, count), bank.seed_counterparties),
        ("counterparty_clients", make_clients(seed), bank.seed_counterparty_clients),
    ):
        for i in range(0, len(items), BATCH):
            counts[key] += await send(items[i : i + BATCH])
    return counts
