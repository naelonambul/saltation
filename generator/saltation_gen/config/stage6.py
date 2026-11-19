"""Stage 6 schedule defaults and synthetic calibration assumptions."""

from dataclasses import dataclass

# Scenario frequencies are fixture assumptions. Lookalikes share one quota.
ANNUAL_RATES = {"A": 10, "B": 10, "C": 5, "D": 5, "E": 15, "F": 15, "G": 10, "lookalike": 100}
LIVE_RATES = dict.fromkeys(ANNUAL_RATES, 2) | {"lookalike": 7}
LOOKALIKE_KINDS = ("travel", "trip", "online", "new_phone", "purchase", "household")

BANKING_AMOUNT_SOURCE = "AI Hub 71925, reported normal electronic banking rows"
# P50/P90/P99 are the source observations. Endpoints and interpolation are synthetic.
BANKING_KRW_ANCHORS = ((0, 1_000), (50, 30_000), (90, 300_000), (99, 1_000_000), (100, 2_000_000))


@dataclass(frozen=True)
class ScheduleConfig:
    """Override quotas are absolute counts. Missing keys in an override have zero quota."""

    population: int = 20_000
    amplification: int = 20
    annual: dict[str, int] | None = None
    live: dict[str, int] | None = None
    normal: bool = True
