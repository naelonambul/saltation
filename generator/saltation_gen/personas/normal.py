"""Stage-1 normal behaviour: a customer sends a small transfer to another
customer at an everyday hour, from one of their usual devices and regions.
Fraud scenarios and lookalikes arrive in stage 5."""

import random
from dataclasses import dataclass
from typing import Any

from saltation_gen.client.bank import Context

REGIONS = ["KR-11", "KR-26", "KR-27", "KR-28", "KR-29", "KR-30", "KR-31", "KR-41"]


@dataclass(frozen=True)
class Step:
    sender: dict[str, Any]
    receiver: dict[str, Any]
    amount: int
    ctx: Context
    memo: str


def home_context(customer: dict[str, Any]) -> Context:
    """Each customer has a stable home device, region and IP."""
    rng = random.Random(customer["customer_no"])
    return Context(
        ip=f"10.{rng.randrange(256)}.{rng.randrange(256)}.{rng.randrange(1, 255)}",
        region=rng.choice(REGIONS),
        device=f"dev-{customer['customer_no']}",
    )


def next_step(rng: random.Random, roster: list[dict[str, Any]]) -> Step:
    sender, receiver = rng.sample(roster, 2)
    amount = rng.choice([5_000, 10_000, 12_000, 20_000, 30_000, 50_000, 100_000])
    memo = rng.choice(["", "점심", "회비", "선물", "용돈", "더치페이"])
    return Step(sender=sender, receiver=receiver, amount=amount, ctx=home_context(sender), memo=memo)
