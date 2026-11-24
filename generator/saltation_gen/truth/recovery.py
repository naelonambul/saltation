"""Durable state for bounded generator proofs on standalone MongoDB."""

from contextlib import suppress
from typing import Any

from pymongo import ReturnDocument
from pymongo.errors import BulkWriteError, DuplicateKeyError
from pymongo.write_concern import WriteConcern

from .store import TruthStore


class RecoveryError(RuntimeError):
    """A run cannot continue without resolving a conflicting or uncertain state."""


class RecoveryJournal:
    def __init__(self, truth: TruthStore, run_id: str, owner: str, *, kind: str = "generator-bc"):
        durability = WriteConcern(w=1, j=True)
        self.runs = truth.runs.with_options(write_concern=durability)
        self.labels = truth.labels.with_options(write_concern=durability)
        self.run_id = run_id
        self.owner = owner
        self.kind = kind

    async def claim(self, inputs: dict[str, Any], stale_owner: str | None) -> dict[str, Any]:
        if stale_owner is None:
            with suppress(DuplicateKeyError):
                await self.runs.insert_one(
                    {"_id": self.run_id, "kind": self.kind, "inputs": inputs, "operations": {}}
                )

        match = {"_id": self.run_id, "kind": self.kind, "inputs": inputs, "owner": stale_owner}
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

    async def save_progress(self, progress: dict[str, Any]) -> None:
        await self._update({"$set": {"progress": progress}})

    async def finish(self, summary: dict[str, Any] | None = None) -> None:
        fields: dict[str, Any] = {"completed": True}
        if summary is not None:
            fields["summary"] = summary
        await self._update({"$set": fields})

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

    async def ensure_labels_batch(self, labels_by_rid: dict[str, list[dict[str, Any]]]) -> None:
        if not labels_by_rid:
            return
        docs = [label for expected in labels_by_rid.values() for label in expected]
        if not docs:
            return
        try:
            await self.labels.insert_many(docs, ordered=False)
            return
        except BulkWriteError as error:
            details = error.details or {}
            if details.get("writeConcernErrors"):
                raise
            write_errors = details.get("writeErrors", [])
            if not write_errors or any(item.get("code") != 11000 for item in write_errors):
                raise
        actual_by_rid: dict[str, list[dict[str, Any]]] = {rid: [] for rid in labels_by_rid}
        async for label in self.labels.find({"request_id": {"$in": list(labels_by_rid)}}, {"_id": 0}):
            actual_by_rid.setdefault(label["request_id"], []).append(label)
        for rid, expected in labels_by_rid.items():
            if sorted(actual_by_rid.get(rid, []), key=_subject) != sorted(expected, key=_subject):
                raise RecoveryError(f"conflicting or incomplete labels for {rid}")


def _subject(label: dict[str, Any]) -> tuple[str, str]:
    return label["subject_type"], label["subject_id"]
