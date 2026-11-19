"""Provision a fresh, journal-fenced Stage 6 backfill world."""

import secrets
from datetime import datetime
from typing import Any

from pymongo.errors import BulkWriteError

from saltation_gen.client import BankClient
from saltation_gen.config import BANK_CODE
from saltation_gen.truth import TruthStore

from .seed import (
    BATCH,
    _fixture_mapping,
    _seed_customer_accounts,
    _valid_synthetic_card,
    make_customers,
    seed_terminals,
)

CARD_CLIENT_ID = "stage6-card"
OOB_CLIENT_ID = "stage6-oob"
SENDER_ACCOUNTS = {
    "salary": "9016000000000001",
    "settlement": "9016000000000002",
    "unrelated": "9016000000000003",
}
_SENDERS = {
    "salary": ("EMPLOYER", "가람 서비스"),
    "settlement": ("MERCHANT", "누리 상점"),
    "unrelated": ("INDIVIDUAL", "한결"),
}
_ANALYST_BATCH = 100


def world_customer_numbers(population: int, start: int = 0) -> list[str]:
    if start < 0 or population < 0 or start + population > 100_000_000:
        raise ValueError("Stage 6 customer slots must fit in eight digits")
    return [f"C{i:08d}" for i in range(start, start + population)]


async def _store_world_credentials(truth: TruthStore, networks: list[dict[str, Any]]) -> None:
    if await truth.card_networks.find_one({"_id": {"$in": [row["_id"] for row in networks]}}, {"_id": 1}):
        raise ValueError("Stage 6 world credentials already reserved")
    try:
        # Fixed, unique IDs claim the world before any bank write. Retain the
        # claim after any failure, including a partial insert or lost response.
        await truth.card_networks.insert_many(networks, ordered=True)
    except BulkWriteError:
        raise RuntimeError("Stage 6 world credential reservation failed") from None


async def _seed_cohort(
    bank: BankClient,
    truth: TruthStore,
    seed: int,
    numbers: list[str],
    start: int,
    dormant: set[str],
    *,
    preparing: bool,
    run_id: str | None,
) -> None:
    for offset in range(0, len(numbers), BATCH):
        slots = numbers[offset : offset + BATCH]
        cohort = {number for number in slots if (number in dormant) == preparing}
        if not cohort:
            continue
        batch_start = start + offset
        batch = [
            row
            for row in make_customers(seed + batch_start, len(slots), batch_start)
            if row["customer_no"] in cohort
        ]
        for row in batch:
            row["opening_balance"] = 0 if preparing else 3_000_000

        expected = {row["account_no"] for row in batch}
        accounts = await _seed_customer_accounts(bank, batch)
        fins = _fixture_mapping(accounts, expected, "fintech_use_num", lambda value: len(value) == 24)
        cards = await bank.seed_cards(
            [{"account_no": row["account_no"], "status": "ACTIVE"} for row in batch]
        )
        card_numbers = _fixture_mapping(cards, expected, "card_no", _valid_synthetic_card)
        if any(card.get("status") != "ACTIVE" for card in cards):
            raise ValueError("Stage 6 card is not ACTIVE")

        try:
            await truth.add_roster(
                [
                    {
                        **row,
                        "fintech_use_num": fins[row["account_no"]],
                        "card_no": card_numbers[row["account_no"]],
                        "protected": True,
                        **({"world_run_id": run_id} if run_id is not None else {}),
                    }
                    for row in batch
                ]
            )
        except BulkWriteError:
            raise RuntimeError("Stage 6 protected roster insert failed") from None


async def seed_world(
    bank: BankClient,
    truth: TruthStore,
    seed: int,
    population: int,
    start: int,
    dormant: set[str],
    preparation_at: datetime,
    first_at: datetime,
    analysts: list[dict] | None = None,
    *,
    run_id: str | None = None,
) -> None:
    """Seed a fresh BACKFILL bank under the caller's exclusive journal fence.

    The caller supplies a preparation date old enough for the dormant cohort.
    A failed call leaves its reservations in place for diagnosis.
    """
    if preparation_at.utcoffset() is None or first_at.utcoffset() is None or first_at < preparation_at:
        raise ValueError("Stage 6 preparation and first workload times must be ordered and timezone-aware")
    numbers = world_customer_numbers(population, start)
    if not dormant.issubset(numbers):
        raise ValueError("Stage 6 dormant cohort contains a customer outside the world")
    analyst_rows = [
        {field: row[field] for field in ("analyst_no", "name", "password")} for row in analysts or []
    ]
    if len({row["analyst_no"] for row in analyst_rows}) != len(analyst_rows):
        raise ValueError("Stage 6 analyst numbers must be unique")

    for offset in range(0, len(numbers), BATCH):
        slots = numbers[offset : offset + BATCH]
        if await truth.roster.find_one(
            {
                "$or": [
                    {"customer_no": {"$in": slots}},
                    {"account_no": {"$in": [f"{BANK_CODE}{number[1:]}" for number in slots]}},
                    {"phone": {"$in": [f"010{number[1:]}" for number in slots]}},
                ]
            },
            {"_id": 1},
        ):
            raise ValueError("Stage 6 roster slots already reserved")

    networks = [
        {"_id": CARD_CLIENT_ID, "client_secret": secrets.token_hex(32)},
        {
            "_id": OOB_CLIENT_ID,
            "client_secret": secrets.token_hex(32),
            "sender_accounts": SENDER_ACCOUNTS.copy(),
        },
    ]
    if run_id is not None:
        for network in networks:
            network["world_run_id"] = run_id
    await _store_world_credentials(truth, networks)

    await bank.clock_set(preparation_at.isoformat())
    await _seed_cohort(bank, truth, seed, numbers, start, dormant, preparing=True, run_id=run_id)
    if first_at > preparation_at:
        # The clock endpoint runs every crossed day's rollover in order.
        await bank.clock_set(first_at.isoformat())
    await _seed_cohort(bank, truth, seed, numbers, start, dormant, preparing=False, run_id=run_id)

    await seed_terminals(bank)
    counterparties = []
    for role, account in SENDER_ACCOUNTS.items():
        kind, name = _SENDERS[role]
        counterparties.append(
            {"institution_code": "901", "account_no": account, "counterparty_type": kind, "name": name}
        )
    if await bank.seed_counterparties(counterparties) != len(counterparties):
        raise RuntimeError("Stage 6 counterparty seed conflicted with existing accounts")
    clients = [
        {
            "client_id": network["_id"],
            "client_secret": network["client_secret"],
            "institution_code": "901",
            "scope": scope,
        }
        for network, scope in zip(networks, ("card", "oob"), strict=True)
    ]
    if await bank.seed_counterparty_clients(clients) != len(clients):
        raise RuntimeError("Stage 6 counterparty client seed conflicted with existing clients")
    for offset in range(0, len(analyst_rows), _ANALYST_BATCH):
        batch = analyst_rows[offset : offset + _ANALYST_BATCH]
        if await bank.seed_analysts(batch) != len(batch):
            raise RuntimeError("Stage 6 analyst seed count did not match the requested batch")
