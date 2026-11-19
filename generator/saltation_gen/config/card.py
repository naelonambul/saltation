"""Card fixture configuration and the assumptions behind its normal payments."""

from random import Random

NORMAL_CARD_ANNUAL_CAP = 100
HOLDER_ACTIVITY_WINDOW_HOURS = 24

# AI Hub dataset 71925, supplied normal card rows, P50/P90/P99 in KRW.
# Positive-only source quantiles are unavailable. The piecewise distribution,
# including its endpoints, is a synthetic fit to reported comparison anchors.
CARD_AMOUNT_SOURCE = "AI Hub 71925, normal card rows, training and validation"
CARD_KRW_ANCHORS = ((0, 1_000), (50, 9_000), (90, 49_000), (99, 113_000), (100, 250_000))
CARD_KRW_TOLERANCES = (1_000, 4_000, 12_000)

# Country, time, and merchant choices in the proof are synthetic assumptions.
# The euro-area membership here is pinned to the checkpoint fixture version.
EURO_AREA = frozenset(
    {
        "AT",
        "BE",
        "BG",
        "HR",
        "CY",
        "EE",
        "FI",
        "FR",
        "DE",
        "GR",
        "IE",
        "IT",
        "LV",
        "LT",
        "LU",
        "MT",
        "NL",
        "PT",
        "SK",
        "SI",
        "ES",
    }
)
FOREIGN_LOCAL_CURRENCIES = {"JP": "JPY", "CN": "CNY"}
KRW_PER_UNIT = {"KRW": 1, "USD": 1_300, "EUR": 1_400, "CNY": 200}
MAX_KRW = 999_999_999_999


def sample_domestic_amount(rng: Random) -> int:
    percentile = rng.random() * 100
    for (low, start), (high, end) in zip(CARD_KRW_ANCHORS, CARD_KRW_ANCHORS[1:], strict=False):
        if percentile < high:
            return round(start + (end - start) * (percentile - low) / (high - low))
    raise AssertionError("percentile must be below 100")


def card_amount_krw(country: str, currency: str, amount: int) -> int:
    if len(country) != 2 or not country.isascii() or not country.isalpha() or not country.isupper():
        raise ValueError("invalid card country code")
    local_currency = "EUR" if country in EURO_AREA else FOREIGN_LOCAL_CURRENCIES.get(country)
    allowed = {"KRW"} if country == "KR" else {"USD", local_currency}
    if currency not in allowed or amount <= 0:
        raise ValueError("invalid card country, currency or amount")
    if currency == "JPY":
        if amount % 100:
            raise ValueError("JPY payments require multiples of 100")
        krw = amount // 100 * 900
    else:
        krw = amount * KRW_PER_UNIT[currency]
    if krw > MAX_KRW:
        raise ValueError("card amount exceeds the ledger bound")
    return krw
