from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from typing import Any

from pymongo import AsyncMongoClient

LABELS = {"normal", "lookalike", "A", "B", "C", "D", "E", "F"}


@dataclass(frozen=True)
class Label:
    """One label for one subject of one request (spec F-34)."""

    request_id: str
    subject_type: str  # "account" or "customer"
    subject_id: str
    label: str
    run_id: str
    scenario: str | None = None
    scenario_instance_id: str | None = None

    def __post_init__(self) -> None:
        if self.subject_type not in ("account", "customer"):
            raise ValueError(f"subject_type must be account or customer, not {self.subject_type}")
        if self.label not in LABELS:
            raise ValueError(f"unknown label {self.label}")


class TruthStore:
    def __init__(self, uri: str):
        self._client: AsyncMongoClient[dict[str, Any]] = AsyncMongoClient(uri)
        db = self._client.get_default_database(default="truth")
        self.labels = db["labels"]
        self.runs = db["runs"]
        self.roster = db["roster"]

    async def setup(self) -> None:
        await self.labels.create_index(
            [("request_id", 1), ("subject_type", 1), ("subject_id", 1)], unique=True, name="label_key"
        )
        await self.roster.create_index("customer_no", unique=True)

    async def close(self) -> None:
        await self._client.close()

    async def label(self, labels: list[Label]) -> None:
        """Written before the request is sent, so a lost response loses no label."""
        if labels:
            await self.labels.insert_many([asdict(x) for x in labels], ordered=False)

    async def start_run(self, run_id: str, **manifest: Any) -> None:
        await self.runs.insert_one({"_id": run_id, "started_at": datetime.now(UTC).isoformat(), **manifest})

    async def finish_run(self, run_id: str, **summary: Any) -> None:
        await self.runs.update_one(
            {"_id": run_id}, {"$set": {"finished_at": datetime.now(UTC).isoformat(), **summary}}
        )

    async def add_roster(self, entries: list[dict[str, Any]]) -> None:
        if entries:
            await self.roster.insert_many(entries, ordered=False)

    async def load_roster(self) -> list[dict[str, Any]]:
        return [doc async for doc in self.roster.find({}, {"_id": 0})]
