"""Deterministic, reserved-subject plan for the bounded card fixture."""

from collections import Counter
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from random import Random
from typing import Any

from saltation_gen.config.card import NORMAL_CARD_ANNUAL_CAP, card_amount_krw, sample_domestic_amount

KST = timezone(timedelta(hours=9))
SUBJECTS = ("g", "trip", "online", "normal")
VERSION = 1


@dataclass(frozen=True)
class Action:
    name: str
    at: datetime
    subject: str
    kind: str
    payload: dict[str, Any]
    decline: bool = False

    @property
    def label(self) -> str:
        return {"g": "G", "normal": "normal"}.get(self.subject, "lookalike")


def proof_time(day: date, hour: int, minute: int) -> datetime:
    return datetime.combine(day, time(hour, minute), KST)


def enforce_normal_card_cap(actions: list[Action], customers: dict[str, str]) -> None:
    counts: Counter[tuple[str, int]] = Counter()
    for action in actions:
        if action.kind == "card" and action.label == "normal":
            customer = customers[action.subject]
            counts[customer, action.at.astimezone(KST).year] += 1
    if any(count > NORMAL_CARD_ANNUAL_CAP for count in counts.values()):
        raise ValueError("normal card actions exceed the annual cap")


def fixture_plan(day: date, seed: int) -> list[Action]:
    def activity(name, hour, minute, subject, kind, **payload):
        return Action(name, proof_time(day, hour, minute), subject, kind, payload)

    def card(name, minute, subject, merchant, amount, *, country="US", online=False, decline=False):
        currency = "KRW" if country == "KR" else "USD"
        payload = {
            "amount": str(amount),
            "currency": currency,
            "country": country,
            "merchant_name": merchant,
            "mcc": "5411",
            "entry_mode": "online" if online else "in_person",
        }
        if not online:
            latitude, longitude = (
                ("37.566500", "126.978000") if country == "KR" else ("40.712800", "-74.006000")
            )
            payload.update(latitude=latitude, longitude=longitude)
        card_amount_krw(country, currency, amount)
        return Action(name, proof_time(day, 10, minute), subject, "card", payload, decline)

    return [
        activity("g_login", 9, 0, "g", "login", region="KR-11"),
        activity("g_atm_start", 9, 1, "g", "atm_start"),
        activity("g_atm_withdraw", 9, 1, "g", "atm_withdraw", amount=10_000),
        activity("trip_login", 10, 0, "trip", "login", region="US-NY"),
        card("g_small", 0, "g", "Harbor Market", 5),
        card("g_large", 1, "g", "Cedar Grocery", 20),
        card("g_decline", 2, "g", "Union Food Hall", 100, decline=True),
        card("trip_card", 3, "trip", "Maple Corner", 20),
        card("online_card", 4, "online", "Northside Foods", 20, online=True),
        card(
            "normal_card",
            5,
            "normal",
            "River Daily Market",
            sample_domestic_amount(Random(seed)),
            country="KR",
        ),
    ]
