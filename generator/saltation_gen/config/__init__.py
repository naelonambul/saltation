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
