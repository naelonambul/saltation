"""Seeds customers and funded demand accounts through the internal interface
(spec F-29) and records them in the generator's roster."""

import hashlib
import random
import re
import secrets
from collections.abc import Callable
from typing import Any

from saltation_gen.client import BankClient
from saltation_gen.config import ATM_TERMINALS
from saltation_gen.truth import TruthStore

FAMILY = ["김", "이", "박", "최", "정", "강", "조", "윤", "장", "임"]
GIVEN = ["민준", "서연", "도윤", "하은", "지호", "수아", "예준", "지우", "현우", "서윤"]
BATCH = 200
CARD_FIXTURE_START = 97_100_000


def make_customers(seed: int, count: int, start: int = 0) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    pin_rng = random.Random(f"{seed}:atm_pin")
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
                "atm_pin": f"{pin_rng.randrange(10000):04d}",
                "account_no": f"998{n}",
                "opening_balance": rng.choice([300_000, 1_000_000, 3_000_000, 10_000_000]),
            }
        )
    return out


async def _seed_customer_accounts(bank: BankClient, batch: list[dict[str, Any]]) -> list[dict[str, Any]]:
    await bank.seed_customers(
        [
            {k: c[k] for k in ("customer_no", "name", "phone", "address", "customer_type", "password")}
            for c in batch
        ]
    )
    return await bank.seed_accounts(
        [{k: c[k] for k in ("customer_no", "account_no", "atm_pin", "opening_balance")} for c in batch]
    )


async def seed(bank: BankClient, truth: TruthStore, seed: int, count: int, start: int = 0) -> int:
    customers = make_customers(seed, count, start)
    for i in range(0, len(customers), BATCH):
        batch = customers[i : i + BATCH]
        accounts = await _seed_customer_accounts(bank, batch)
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


async def seed_terminals(bank: BankClient) -> int:
    return await bank.seed_atm_terminals(ATM_TERMINALS)


CARD_NETWORK_ID = "card-network"


def _fixture_mapping(
    rows: list[dict[str, Any]], expected: set[str], field: str, valid: Callable[[str], bool]
) -> dict[str, str]:
    mapping: dict[str, str] = {}
    if not isinstance(rows, list):
        raise ValueError(f"invalid fixture mapping for {field}")
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError(f"invalid fixture mapping for {field}")
        account, value = row.get("account_no"), row.get(field)
        if not isinstance(account, str) or account not in expected or account in mapping:
            raise ValueError(f"unknown, malformed or duplicate fixture account for {field}")
        if not isinstance(value, str) or not valid(value):
            raise ValueError(f"invalid fixture {field}")
        mapping[account] = value
    if mapping.keys() != expected or len(set(mapping.values())) != len(expected):
        raise ValueError(f"incomplete or duplicate fixture mapping for {field}")
    return mapping


def _valid_synthetic_card(card_no: str) -> bool:
    if not re.fullmatch(r"0000[0-9]{12}", card_no):
        return False
    # Luhn doubles every other digit, starting immediately before the check digit.
    doubled = sum(sum(divmod(2 * int(digit), 10)) for digit in card_no[::2])
    return (doubled + sum(map(int, card_no[1::2]))) % 10 == 0


async def seed_card_fixture(bank: BankClient, truth: TruthStore, seed_value: int) -> None:
    """Seed the dedicated card fixture after the harness resets the bank."""
    balances = (100_000, 1_000_000, 1_000_000, 1_000_000)
    customers = make_customers(seed_value, len(balances), CARD_FIXTURE_START)
    for customer, balance in zip(customers, balances, strict=True):
        customer["opening_balance"] = balance

    expected = {c["account_no"] for c in customers}
    accounts = await _seed_customer_accounts(bank, customers)
    fins = _fixture_mapping(accounts, expected, "fintech_use_num", lambda value: len(value) == 24)
    cards = await bank.seed_cards([{"account_no": c["account_no"], "status": "ACTIVE"} for c in customers])
    numbers = _fixture_mapping(cards, expected, "card_no", _valid_synthetic_card)
    if any(card.get("status") != "ACTIVE" for card in cards):
        raise ValueError("fixture card is not ACTIVE")

    await seed_terminals(bank)
    client_secret = secrets.token_hex(32)
    await bank.seed_counterparty_clients(
        [
            {
                "client_id": CARD_NETWORK_ID,
                "client_secret": client_secret,
                "institution_code": "901",
                "scope": "card",
            }
        ]
    )
    await truth.card_networks.replace_one(
        {"_id": CARD_NETWORK_ID}, {"_id": CARD_NETWORK_ID, "client_secret": client_secret}, upsert=True
    )
    await truth.roster.delete_many({"customer_no": {"$in": [c["customer_no"] for c in customers]}})
    await truth.add_roster(
        [
            {**c, "fintech_use_num": fins[c["account_no"]], "card_no": numbers[c["account_no"]]}
            for c in customers
        ]
    )
