"""Durable request state for the bounded B/C proof on standalone MongoDB."""

from contextlib import suppress
from typing import Any

from pymongo import ReturnDocument
from pymongo.errors import DuplicateKeyError
from pymongo.write_concern import WriteConcern

from .store import TruthStore


class RecoveryError(RuntimeError):
    """A run cannot continue without resolving a conflicting or uncertain state."""


class RecoveryJournal:
    def __init__(self, truth: TruthStore, run_id: str, owner: str):
        durability = WriteConcern(w=1, j=True)
        self.runs = truth.runs.with_options(write_concern=durability)
        self.labels = truth.labels.with_options(write_concern=durability)
        self.run_id = run_id
        self.owner = owner

    async def claim(self, inputs: dict[str, Any], stale_owner: str | None) -> dict[str, Any]:
        if stale_owner is None:
            with suppress(DuplicateKeyError):
                await self.runs.insert_one(
                    {"_id": self.run_id, "kind": "generator-bc", "inputs": inputs, "operations": {}}
                )

        match = {"_id": self.run_id, "kind": "generator-bc", "inputs": inputs, "owner": stale_owner}
        run = await self.runs.find_one_and_update(
            match, {"$set": {"owner": self.owner}}, return_document=ReturnDocument.AFTER
        )
        if run is None:
            raise RecoveryError("run inputs differ, the run already has an owner, or the claim is stale")
        return run

    async def release(self) -> None:
        await self._update({"$unset": {"owner": ""}})

    async def prepare(self, name: str, operation: dict[str, Any]) -> None:
        path = f"operations.{name}"
        await self._update({"$set": {path: operation}}, {path: {"$exists": False}})

    async def transition(self, name: str, old: str, new: str) -> None:
        path = f"operations.{name}.state"
        await self._update({"$set": {path: new}}, {path: old})

    async def finish(self) -> None:
        await self._update({"$set": {"completed": True}})

    async def _update(self, update: dict[str, Any], conditions: dict[str, Any] | None = None) -> None:
        query = {"_id": self.run_id, "owner": self.owner, **(conditions or {})}
        result = await self.runs.update_one(query, update)
        if result.matched_count != 1:
            raise RecoveryError("the run owner or operation state changed")

    async def ensure_labels(self, request_id: str, expected: list[dict[str, Any]]) -> None:
        for label in expected:
            key = {field: label[field] for field in ("request_id", "subject_type", "subject_id")}
            # A concurrent insert may win the unique key. Read back before trusting it.
            with suppress(DuplicateKeyError):
                await self.labels.update_one(key, {"$setOnInsert": label}, upsert=True)
        actual = [label async for label in self.labels.find({"request_id": request_id}, {"_id": 0})]
        if sorted(actual, key=_subject) != sorted(expected, key=_subject):
            raise RecoveryError(f"conflicting or incomplete labels for {request_id}")


def _subject(label: dict[str, Any]) -> tuple[str, str]:
    return label["subject_type"], label["subject_id"]
