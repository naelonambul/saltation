"""Generator settings from the environment."""

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Settings:
    bank_url: str
    truth_uri: str
    seed: int

    @classmethod
    def from_env(cls) -> "Settings":
        return cls(
            bank_url=os.environ.get("SALTATION_BANK_URL", "http://bank:8081"),
            truth_uri=os.environ["MONGO_TRUTH_URI"],
            seed=int(os.environ.get("SALTATION_GEN_SEED", "20261112")),
        )


# Saltation's synthetic bank_code_std (contracts/codes/institutions.json).
BANK_CODE = "998"
# Spec F-36a: pause while more events than this are undelivered; resume below RESUME.
BACKPRESSURE_PAUSE = 50_000
BACKPRESSURE_RESUME = 10_000
