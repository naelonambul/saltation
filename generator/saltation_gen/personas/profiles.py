"""Synthetic normal behavior profiles. Frequencies and contexts are fixture assumptions."""

from dataclasses import dataclass
from hashlib import blake2b
from random import Random

from saltation_gen.config.card import NORMAL_CARD_ANNUAL_CAP, sample_domestic_amount
from saltation_gen.config.stage6 import BANKING_KRW_ANCHORS
from saltation_gen.personas.normal import REGIONS

# Requests include authentication on transfers, session creation at the ATM,
# combined mobile visits (login + balance + transfer reusing one F-2 session),
# and standalone login/balance episodes. Income and card each use one request.
REQUESTS_PER_EPISODE = {
    "normal_transfer": 2,
    "normal_visit": 3,
    "normal_income": 1,
    "normal_card": 1,
    "normal_atm": 2,
    "normal_login_balance": 2,
}
NORMAL_SCHEDULE_KINDS = (
    "normal_transfer",
    "normal_income",
    "normal_card",
    "normal_atm",
    "normal_login_balance",
)
# Share of normal transfer days combined with a balance inquiry into one F-2 session (spec F-32).
# Keeping 25% standalone login->transfer avoids leaking ground-truth labels via balance presence.
NORMAL_VISIT_RATIO = 0.75


@dataclass(frozen=True)
class Cadence:
    annual: int
    hour: int


@dataclass(frozen=True)
class Profile:
    transfer: Cadence
    income: Cadence
    card: Cadence
    atm: Cadence
    login_balance: Cadence
    income_activity: str
    income_krw: int
    cash_krw: int
    devices: tuple[str, ...]
    location_count: int

    def cadence(self, kind: str) -> Cadence:
        if kind == "normal_visit":
            return self.transfer
        return getattr(self, kind.removeprefix("normal_"))

    @property
    def visit_annual(self) -> int:
        return round(self.transfer.annual * NORMAL_VISIT_RATIO)

    @property
    def transfer_request_cost(self) -> int:
        standalone = self.transfer.annual - self.visit_annual
        return (
            standalone * REQUESTS_PER_EPISODE["normal_transfer"]
            + self.visit_annual * REQUESTS_PER_EPISODE["normal_visit"]
        )

    @property
    def annual_requests(self) -> int:
        return (
            self.transfer_request_cost
            + self.income.annual * REQUESTS_PER_EPISODE["normal_income"]
            + self.card.annual * REQUESTS_PER_EPISODE["normal_card"]
            + self.atm.annual * REQUESTS_PER_EPISODE["normal_atm"]
            + self.login_balance.annual * REQUESTS_PER_EPISODE["normal_login_balance"]
        )

    def amount(self, kind: str, rng: Random) -> int:
        if kind == "normal_income":
            return self.income_krw
        if kind == "normal_atm":
            return self.cash_krw
        if kind == "normal_card":
            return sample_domestic_amount(rng)
        if kind not in {"normal_transfer", "normal_visit"}:
            raise ValueError(f"no amount for {kind}")

        percentile = rng.random() * 100
        for (low, start), (high, end) in zip(BANKING_KRW_ANCHORS, BANKING_KRW_ANCHORS[1:], strict=False):
            if percentile < high:
                return round(start + (end - start) * (percentile - low) / (high - low))
        raise AssertionError("percentile must be below 100")

    def context(self, seed: int, customer: str, occurrence: str) -> tuple[str, str]:
        """Return a seeded familiar device ID and domestic region for this occurrence."""
        home = stable_number(seed, customer) % len(REGIONS)
        activity = stable_number(seed, occurrence)
        device = self.devices[activity % len(self.devices)]
        region = REGIONS[(home + (activity // len(self.devices)) % self.location_count) % len(REGIONS)]
        return f"dev-{customer}-{device}", region


def stable_number(*parts: int | str) -> int:
    """Stable across processes and independent of Python's hash randomization."""
    digest = blake2b(repr(parts).encode(), digest_size=8)
    return int.from_bytes(digest.digest())


# One cadence per activity gives each group a distinct mix and local-time pattern.
# Standalone login_balance cadences exclude the visits combined with normal_transfer (spec F-32).
PROFILES = {
    "working": Profile(
        Cadence(60, 18),
        Cadence(12, 8),
        Cadence(96, 12),
        Cadence(24, 17),
        Cadence(55, 19),
        "salary",
        3_100_000,
        50_000,
        ("phone", "laptop"),
        2,
    ),
    "student": Profile(
        Cadence(65, 20),
        Cadence(12, 10),
        Cadence(98, 14),
        Cadence(12, 16),
        Cadence(56, 21),
        "settlement",
        450_000,
        30_000,
        ("phone", "tablet"),
        2,
    ),
    "sole_trader": Profile(
        Cadence(68, 15),
        Cadence(52, 7),
        Cadence(88, 13),
        Cadence(32, 11),
        Cadence(17, 17),
        "settlement",
        850_000,
        100_000,
        ("phone", "terminal", "laptop"),
        3,
    ),
    "older": Profile(
        Cadence(50, 11),
        Cadence(12, 9),
        Cadence(60, 10),
        Cadence(48, 14),
        Cadence(66, 16),
        "settlement",
        1_200_000,
        80_000,
        ("phone",),
        1,
    ),
}

if any(profile.card.annual > NORMAL_CARD_ANNUAL_CAP for profile in PROFILES.values()):
    raise ValueError("normal card cadence exceeds the KST calendar-year cap")
