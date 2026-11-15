"""Generator settings from the environment."""

import os
from dataclasses import dataclass
from urllib.parse import quote


@dataclass(frozen=True)
class Settings:
    bank_url: str
    truth_uri: str
    seed: int

    @classmethod
    def from_env(cls) -> "Settings":
        return cls(
            bank_url=os.environ.get("SALTATION_BANK_URL", "http://bank:8081"),
            truth_uri=_truth_uri(),
            seed=int(os.environ.get("SALTATION_GEN_SEED", "20261112")),
        )


# Saltation's synthetic bank_code_std (contracts/codes/institutions.json).
BANK_CODE = "998"
# Spec F-36a: pause while more events than this are undelivered; resume below RESUME.
BACKPRESSURE_PAUSE = 50_000
BACKPRESSURE_RESUME = 10_000


def _truth_uri() -> str:
    """MONGO_TRUTH_URI, or built from MONGO_HOST, MONGO_USER, MONGO_PASSWORD and
    MONGO_DB so no credential sits inside a configured URI."""
    if uri := os.environ.get("MONGO_TRUTH_URI"):
        return uri
    user = quote(os.environ["MONGO_USER"], safe="")
    password = quote(os.environ["MONGO_PASSWORD"], safe="")
    return f"mongodb://{user}:{password}@{os.environ['MONGO_HOST']}/{os.environ.get('MONGO_DB', 'truth')}"


# Synthetic city-centre terminals, one per real Korean ISO 3166-2 region.
ATM_TERMINALS = [
    {
        "terminal_id": "ATM-11-001",
        "region": "KR-11",
        "location": "서울",
        "latitude": "37.566500",
        "longitude": "126.978000",
    },
    {
        "terminal_id": "ATM-26-001",
        "region": "KR-26",
        "location": "부산",
        "latitude": "35.179800",
        "longitude": "129.075000",
    },
    {
        "terminal_id": "ATM-27-001",
        "region": "KR-27",
        "location": "대구",
        "latitude": "35.871400",
        "longitude": "128.601400",
    },
    {
        "terminal_id": "ATM-28-001",
        "region": "KR-28",
        "location": "인천",
        "latitude": "37.456300",
        "longitude": "126.705200",
    },
    {
        "terminal_id": "ATM-29-001",
        "region": "KR-29",
        "location": "광주",
        "latitude": "35.160100",
        "longitude": "126.851400",
    },
    {
        "terminal_id": "ATM-30-001",
        "region": "KR-30",
        "location": "대전",
        "latitude": "36.350400",
        "longitude": "127.384500",
    },
    {
        "terminal_id": "ATM-31-001",
        "region": "KR-31",
        "location": "울산",
        "latitude": "35.538400",
        "longitude": "129.311400",
    },
    {
        "terminal_id": "ATM-41-001",
        "region": "KR-41",
        "location": "수원",
        "latitude": "37.263600",
        "longitude": "127.028600",
    },
    {
        "terminal_id": "ATM-42-001",
        "region": "KR-42",
        "location": "춘천",
        "latitude": "37.881300",
        "longitude": "127.729800",
    },
    {
        "terminal_id": "ATM-43-001",
        "region": "KR-43",
        "location": "청주",
        "latitude": "36.642400",
        "longitude": "127.489000",
    },
    {
        "terminal_id": "ATM-44-001",
        "region": "KR-44",
        "location": "홍성",
        "latitude": "36.601400",
        "longitude": "126.660800",
    },
    {
        "terminal_id": "ATM-45-001",
        "region": "KR-45",
        "location": "전주",
        "latitude": "35.824200",
        "longitude": "127.148000",
    },
    {
        "terminal_id": "ATM-46-001",
        "region": "KR-46",
        "location": "무안",
        "latitude": "34.990500",
        "longitude": "126.481700",
    },
    {
        "terminal_id": "ATM-47-001",
        "region": "KR-47",
        "location": "안동",
        "latitude": "36.568400",
        "longitude": "128.729400",
    },
    {
        "terminal_id": "ATM-48-001",
        "region": "KR-48",
        "location": "창원",
        "latitude": "35.228000",
        "longitude": "128.681100",
    },
    {
        "terminal_id": "ATM-49-001",
        "region": "KR-49",
        "location": "제주",
        "latitude": "33.499600",
        "longitude": "126.531200",
    },
    {
        "terminal_id": "ATM-50-001",
        "region": "KR-50",
        "location": "세종",
        "latitude": "36.480000",
        "longitude": "127.289000",
    },
]
