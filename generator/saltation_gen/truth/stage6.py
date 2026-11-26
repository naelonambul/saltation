"""Journal for a stage 6 run against a standalone truth database."""

import asyncio
import re
from collections import Counter
from contextlib import suppress
from datetime import datetime
from typing import Any
from uuid import uuid4

from pymongo import ReturnDocument, UpdateOne
from pymongo.errors import BulkWriteError, DuplicateKeyError
from pymongo.write_concern import WriteConcern

from .recovery import RecoveryError
from .store import TruthStore


def _compatible_live_attach(existing_inputs: dict, new_inputs: dict, progress: dict) -> bool:
    """Allow attaching/updating live_at once backfill is finished and before LIVE starts."""
    if (
        not progress.get("backfill_finished")
        or progress.get("live_started") is not None
        or progress.get("live_intended")
    ):
        return False
    existing_run = existing_inputs.get("run")
    new_run = new_inputs.get("run")
    if not isinstance(existing_run, dict) or not isinstance(new_run, dict):
        return False
    new_live_at = new_run.get("live_at")
    if not isinstance(new_live_at, str) or not new_live_at or new_live_at == existing_run.get("live_at"):
        return False
    if existing_inputs.get("backfill_end") != new_inputs.get("backfill_end"):
        return False
    try:
        if datetime.fromisoformat(new_live_at) < datetime.fromisoformat(existing_inputs["backfill_end"]):
            return False
    except (KeyError, TypeError, ValueError):
        return False
    existing_counts = existing_inputs.get("planned_counts", {})
    new_counts = new_inputs.get("planned_counts", {})
    if not isinstance(existing_counts, dict) or not isinstance(new_counts, dict):
        return False
    if {k: v for k, v in existing_counts.items() if k != "live"} != {
        k: v for k, v in new_counts.items() if k != "live"
    }:
        return False
    live_fields = {"run", "schedule_digest", "planned_counts"}
    if {k: v for k, v in existing_inputs.items() if k not in live_fields} != {
        k: v for k, v in new_inputs.items() if k not in live_fields
    }:
        return False
    return {k: v for k, v in existing_run.items() if k != "live_at"} == {
        k: v for k, v in new_run.items() if k != "live_at"
    }


def _normal_day_regex(days: list[str]) -> str:
    joined = "|".join(re.escape(day) for day in days)
    return f"^normal_[^:]+:({joined}):"


_BANK_LEASE = "bank"
_TERMINAL = ("succeeded", "declined")
_SECRET_FIELDS = {
    "access_token",
    "atm_pin",
    "atm_session_token",
    "auth",
    "authorization",
    "card_token",
    "code",
    "password",
    "pin",
    "refresh_token",
    "reset_code",
    "secret",
    "session_token",
    "sms_code",
    "token",
}
_GENERATED_FIELDS = {"request_id", "request_ids", "request_key", "idempotency_key"}
_ROW_FIELDS = {"_id", "run_id", "episode_id", "name", "owner", "state", "result"}


def _check_safe(value: Any) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower().replace("-", "_") in _SECRET_FIELDS:
                raise RecoveryError("credential fields cannot be stored in the stage 6 journal")
            _check_safe(item)
    elif isinstance(value, (list, tuple)):
        for item in value:
            _check_safe(item)


def _stable_shape(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: _stable_shape(item) for key, item in value.items() if key not in _GENERATED_FIELDS}
    if isinstance(value, (list, tuple)):
        return [_stable_shape(item) for item in value]
    return value


class Stage6Journal:
    def __init__(self, truth: TruthStore, run_id: str, owner: str):
        if not owner:
            raise ValueError("stage 6 requires a nonempty owner token")
        self.truth = truth
        self.run_id = run_id
        self.owner = owner
        durability = WriteConcern(w=1, j=True)
        database = truth.runs.database
        self._runs = truth.runs.with_options(write_concern=durability)
        self._operations = database["stage6_operations"].with_options(write_concern=durability)
        self._leases = database["stage6_leases"].with_options(write_concern=durability)
        self._credit = database["stage6_credit"].with_options(write_concern=durability)
        self._lock = asyncio.Lock()
        self._claimed = False
        self._claim_id = ""

    def _run_key(self) -> dict:
        return {"_id": self.run_id, "kind": "generator-stage6"}

    def _lease_key(self) -> dict:
        return {
            "_id": _BANK_LEASE,
            "run_id": self.run_id,
            "owner": self.owner,
            "claim_id": self._claim_id,
        }

    def _owned_run_key(self) -> dict:
        return self._run_key() | {"owner": self.owner, "stage6_claim_id": self._claim_id}

    def _operation_key(self, episode_id: str, name: str) -> dict:
        return {
            "_id": {"run_id": self.run_id, "episode_id": episode_id, "name": name},
            "run_id": self.run_id,
        }

    async def claim(
        self, inputs: dict, exited_owner: str | None = None, *, provenance: dict | None = None
    ) -> dict:
        """Resume a run, supplying an exact owner token only after confirming its process exited."""
        _check_safe(inputs)
        if provenance is not None:
            _check_safe(provenance)
        async with self._lock:
            if self._claimed or exited_owner == self.owner:
                raise RecoveryError("claim requires a fresh stage 6 owner token")

            lease = await self._leases.find_one({"_id": _BANK_LEASE})
            if lease is None:
                if exited_owner is not None:
                    raise RecoveryError("stage 6 bank lease no longer matches the exited owner")
            elif exited_owner is None or lease["owner"] != exited_owner or lease["run_id"] != self.run_id:
                raise RecoveryError("stage 6 bank lease belongs to another owner or run")

            run = await self._runs.find_one({"_id": self.run_id})
            if run is None:
                with suppress(DuplicateKeyError):
                    await self._runs.insert_one(self._run_key() | {"inputs": inputs})
                run = await self._runs.find_one({"_id": self.run_id})
            if run is None or run.get("kind") != "generator-stage6":
                raise RecoveryError("stage 6 run kind or inputs differ")
            live_attached = False
            stored_inputs = run.get("inputs")
            if stored_inputs != inputs:
                if not isinstance(stored_inputs, dict):
                    raise RecoveryError("stage 6 run kind or inputs differ")
                if _compatible_live_attach(stored_inputs, inputs, run.get("progress", {})):
                    live_attached = True
                else:
                    differences = [
                        f"{key} (stored={stored_inputs.get(key)!r}, computed={inputs.get(key)!r})"
                        for key in (
                            "version",
                            "workload_version",
                            "schedule_digest",
                            "backfill_schedule_digest",
                        )
                        if stored_inputs.get(key) != inputs.get(key)
                    ]
                    details = f": {', '.join(differences)}" if differences else ""
                    raise RecoveryError(f"stage 6 run kind or inputs differ{details}")
            prior_owner = run.get("owner")
            if prior_owner is not None and (
                lease is None or prior_owner not in (lease["owner"], lease.get("prior_owner"))
            ):
                raise RecoveryError("stage 6 run has an owner outside the bank lease")

            # The lease records the run owner observed before changing the lease. If the
            # claimant exits between those writes, its successor can still fence that owner.
            self._claim_id = uuid4().hex
            if lease is None:
                try:
                    await self._leases.insert_one(self._lease_key() | {"prior_owner": prior_owner})
                except DuplicateKeyError:
                    raise RecoveryError("stage 6 bank lease was acquired by another owner") from None
            else:
                acquired = await self._leases.update_one(
                    {
                        "_id": _BANK_LEASE,
                        "run_id": self.run_id,
                        "owner": exited_owner,
                        "claim_id": lease["claim_id"],
                    },
                    {"$set": {"owner": self.owner, "claim_id": self._claim_id, "prior_owner": prior_owner}},
                )
                if acquired.matched_count != 1:
                    raise RecoveryError("stage 6 bank lease changed during takeover")

            run_set: dict[str, Any] = {"owner": self.owner, "stage6_claim_id": self._claim_id}
            if provenance is not None:
                run_set["provenance"] = provenance
            run_unset: dict[str, str] = {}
            if live_attached:
                run_set["inputs"] = inputs
                if run.get("completed") and run.get("progress", {}).get("live_started") is None:
                    run_unset["completed"] = ""
                    run_unset["summary"] = ""
            run_update: dict[str, Any] = {"$set": run_set}
            if run_unset:
                run_update["$unset"] = run_unset
            claimed = await self._runs.find_one_and_update(
                self._run_key()
                | {
                    "inputs": {"$eq": run["inputs"]},
                    "owner": prior_owner,
                    "stage6_claim_id": run.get("stage6_claim_id"),
                },
                run_update,
                return_document=ReturnDocument.AFTER,
            )
            if claimed is None:
                raise RecoveryError("stage 6 run changed during claim")

            await self._operations.create_index(
                [("run_id", 1), ("state", 1), ("episode_id", 1), ("name", 1)],
                name="stage6_run_state_episode",
            )
            await self._credit.create_index([("run_id", 1), ("window", 1), ("kind", 1)])
            self._claimed = True
            return claimed

    async def _require_lease(self) -> None:
        if not self._claimed or await self._leases.find_one(self._lease_key(), {"_id": 1}) is None:
            raise RecoveryError("stage 6 bank lease is no longer held")

    async def _require_owner(self) -> None:
        await self._require_lease()
        run = await self._runs.find_one(self._owned_run_key(), {"completed": 1})
        if run is None or run.get("completed"):
            raise RecoveryError("stage 6 run is complete or its owner changed")

    async def _update_run(self, update: dict) -> None:
        changed = await self._runs.update_one(self._owned_run_key(), update)
        if changed.matched_count != 1:
            raise RecoveryError("stage 6 run owner changed")

    async def release(self) -> None:
        async with self._lock:
            if not self._claimed or await self._leases.find_one(self._lease_key()) is None:
                raise RecoveryError("stage 6 bank lease is no longer held")
            # Free the run first, keeping the bank lease as a barrier until it is safe
            # for another run to claim it. The null match also allows retrying release.
            changed = await self._runs.update_one(
                self._owned_run_key() | {"owner": {"$in": [self.owner, None]}},
                {"$unset": {"owner": ""}},
            )
            if changed.matched_count != 1:
                raise RecoveryError("stage 6 run owner changed")
            released = await self._leases.delete_one(self._lease_key())
            if released.deleted_count != 1:
                raise RecoveryError("stage 6 bank lease changed during release")
            self._claimed = False

    async def save_progress(self, progress: dict) -> None:
        _check_safe(progress)
        async with self._lock:
            await self._require_owner()
            await self._update_run({"$set": {"progress": progress}})

    async def get_operation(self, episode_id: str, name: str) -> dict | None:
        return await self._operations.find_one(self._operation_key(episode_id, name))

    async def prepare(self, episode_id: str, name: str, record: dict) -> dict:
        if record.keys() & (_ROW_FIELDS - {"state"}) or record.get("state", "prepared") != "prepared":
            raise RecoveryError("invalid stage 6 prepared operation")
        _check_safe(record)
        async with self._lock:
            await self._require_owner()
            row = await self.get_operation(episode_id, name)
            if row is None:
                row = (
                    record
                    | self._operation_key(episode_id, name)
                    | {
                        "episode_id": episode_id,
                        "name": name,
                        "owner": self.owner,
                        "state": "prepared",
                    }
                )
                try:
                    await self._operations.insert_one(row)
                except DuplicateKeyError:
                    row = await self.get_operation(episode_id, name)
                else:
                    return row
            if row is None or _stable_shape({k: v for k, v in row.items() if k not in _ROW_FIELDS}) != (
                _stable_shape({k: v for k, v in record.items() if k != "state"})
            ):
                raise RecoveryError("stage 6 prepared operation differs from the journal")
            return row

    async def retime(self, episode_id: str, name: str, request_id: str, business_time: str) -> None:
        async with self._lock:
            await self._require_owner()
            result = await self._operations.update_one(
                self._operation_key(episode_id, name) | {"state": "prepared", "request_id": request_id},
                {"$set": {"business_time": business_time}},
            )
            if result.matched_count != 1:
                raise RecoveryError("only previously unsent operations can be retimed")

    async def transition(
        self,
        episode_id: str,
        name: str,
        old: str,
        new: str,
        result: dict | None = None,
    ) -> None:
        """Persist a state change. Dispatch only after prepared -> unconfirmed succeeds."""
        if old == new or old in _TERMINAL or new == "prepared":
            raise RecoveryError("invalid stage 6 state transition")
        _check_safe(result)
        async with self._lock:
            await self._require_owner()
            row = await self.get_operation(episode_id, name)
            if row is None:
                raise RecoveryError("stage 6 operation is missing")
            # The bank lease excludes live previous owners. Adopt their row only in
            # the same CAS that changes state, including at the pre-dispatch boundary.
            fields = {"owner": self.owner, "state": new}
            if result is not None:
                fields["result"] = result
            changed = await self._operations.update_one(
                self._operation_key(episode_id, name) | {"owner": row["owner"], "state": old},
                {"$set": fields},
            )
            if changed.matched_count != 1:
                raise RecoveryError("stage 6 operation owner or state changed")

    async def prepare_batch(self, items: list[tuple[str, str, dict]]) -> dict[tuple[str, str], dict]:
        """Durably prepare a bounded chunk of operations in a single journaled batch."""
        if not items:
            return {}
        rows: list[dict] = []
        for episode_id, name, record in items:
            if record.keys() & (_ROW_FIELDS - {"state"}) or record.get("state", "prepared") != "prepared":
                raise RecoveryError("invalid stage 6 prepared operation")
            _check_safe(record)
            rows.append(
                record
                | self._operation_key(episode_id, name)
                | {
                    "episode_id": episode_id,
                    "name": name,
                    "owner": self.owner,
                    "state": "prepared",
                }
            )
        async with self._lock:
            await self._require_lease()
            try:
                await self._operations.insert_many(rows, ordered=False)
            except BulkWriteError as error:
                details = error.details or {}
                if details.get("writeConcernErrors"):
                    raise
                write_errors = details.get("writeErrors", [])
                if not write_errors or any(item.get("code") != 11000 for item in write_errors):
                    raise
                ids = [row["_id"] for row in rows]
                existing_by_key: dict[tuple[str, str], dict] = {}
                async for doc in self._operations.find({"_id": {"$in": ids}, "run_id": self.run_id}):
                    existing_by_key[(doc["episode_id"], doc["name"])] = doc
                result: dict[tuple[str, str], dict] = {}
                for episode_id, name, record in items:
                    existing = existing_by_key.get((episode_id, name))
                    if existing is None:
                        raise RecoveryError(
                            f"stage 6 prepared operation missing for {episode_id}/{name}"
                        ) from None
                    if existing["state"] in _TERMINAL:
                        result[(episode_id, name)] = existing
                        continue
                    if existing["state"] != "prepared":
                        raise RecoveryError(f"unresolved stage 6 operation {episode_id}/{name}") from None
                    if _stable_shape({k: v for k, v in existing.items() if k not in _ROW_FIELDS}) != (
                        _stable_shape({k: v for k, v in record.items() if k != "state"})
                    ):
                        raise RecoveryError("stage 6 prepared operation differs from the journal") from None
                    result[(episode_id, name)] = existing
                return result
            return {(row["episode_id"], row["name"]): row for row in rows}

    async def mark_unconfirmed_wave(self, rows: list[dict]) -> None:
        """Atomically fence and transition a wave's prepared rows to unconfirmed before HTTP dispatch."""
        if not rows:
            return
        async with self._lock:
            await self._require_lease()
            owners = {row["owner"] for row in rows}
            if len(owners) == 1:
                owner = next(iter(owners))
                ids = [self._operation_key(row["episode_id"], row["name"])["_id"] for row in rows]
                changed = await self._operations.update_many(
                    {"_id": {"$in": ids}, "run_id": self.run_id, "owner": owner, "state": "prepared"},
                    {"$set": {"owner": self.owner, "state": "unconfirmed"}},
                )
            else:
                ops = [
                    UpdateOne(
                        self._operation_key(row["episode_id"], row["name"])
                        | {"owner": row["owner"], "state": "prepared"},
                        {"$set": {"owner": self.owner, "state": "unconfirmed"}},
                    )
                    for row in rows
                ]
                changed = await self._operations.bulk_write(ops, ordered=False)
            if changed.matched_count != len(rows):
                raise RecoveryError("stage 6 operation owner or state changed")

    async def complete_wave(self, outcomes: list[tuple[str, str, str, dict]]) -> None:
        """Persist terminal outcomes for all confirmed operations in a wave in one journaled batch."""
        if not outcomes:
            return
        ops: list[UpdateOne] = []
        for episode_id, name, terminal, result in outcomes:
            if terminal not in _TERMINAL:
                raise RecoveryError("invalid stage 6 state transition")
            _check_safe(result)
            ops.append(
                UpdateOne(
                    self._operation_key(episode_id, name) | {"owner": self.owner, "state": "unconfirmed"},
                    {"$set": {"owner": self.owner, "state": terminal, "result": result}},
                )
            )
        async with self._lock:
            await self._require_lease()
            changed = await self._operations.bulk_write(ops, ordered=False)
            if changed.matched_count != len(outcomes):
                raise RecoveryError("stage 6 operation owner or state changed")

    async def unresolved(self) -> bool:
        return (
            await self._operations.find_one(
                {"run_id": self.run_id, "state": {"$nin": ["prepared", *_TERMINAL]}},
                {"_id": 1},
            )
            is not None
        )

    async def require_success(
        self,
        episode_id: str,
        names: list[str],
        *,
        expected_declined: list[str] | None = None,
    ) -> None:
        declined_expected = set(expected_declined or ())
        succeeded_expected = set(names) - declined_expected
        actual: dict[str, str] = {
            row["name"]: row["state"]
            async for row in self._operations.find(
                {
                    "run_id": self.run_id,
                    "episode_id": episode_id,
                    "state": {"$in": list(_TERMINAL)},
                    "name": {"$in": names},
                },
                {"name": 1, "state": 1},
            )
        }
        succeeded_actual = {name for name, state in actual.items() if state == "succeeded"}
        declined_actual = {name for name, state in actual.items() if state == "declined"}
        if succeeded_actual != succeeded_expected or declined_actual != declined_expected:
            raise RecoveryError("scenario credit requires every scheduled operation to succeed")

    async def credit(self, window: str, kind: str, identity: str, episode_id: str) -> None:
        key = {"run_id": self.run_id, "window": window, "kind": kind, "identity": identity}
        async with self._lock:
            await self._require_owner()
            terminal = await self._operations.find_one(
                {"run_id": self.run_id, "state": "succeeded", "episode_id": episode_id},
                {"_id": 1},
            )
            if terminal is None:
                raise RecoveryError("stage 6 quota credit requires a terminal operation")
            with suppress(DuplicateKeyError):
                await self._credit.update_one(
                    {"_id": key, "run_id": self.run_id},
                    {"$setOnInsert": key | {"episode_id": episode_id}},
                    upsert=True,
                )

    async def prune_completed_normal_day(self, day_iso: str) -> int:
        """Archive and delete terminal normal_* operations for a finished backfill day."""
        filt = {
            "run_id": self.run_id,
            "episode_id": {"$regex": _normal_day_regex([day_iso])},
            "state": {"$in": list(_TERMINAL)},
        }
        async with self._lock:
            await self._require_owner()
            run = await self._runs.find_one(self._owned_run_key(), {"archived_normal": 1})
            archived = (run or {}).get("archived_normal") or {}
            if day_iso not in archived:
                cursor = await self._operations.aggregate(
                    [
                        {"$match": filt},
                        {
                            "$group": {
                                "_id": {
                                    "class": "$scenario_class",
                                    "persona": "$persona",
                                    "state": "$state",
                                    "carry_in": "$prior_to_live",
                                },
                                "count": {"$sum": 1},
                            }
                        },
                    ]
                )
                rollup = [
                    {
                        "class": row["_id"].get("class", "unclassified"),
                        "persona": row["_id"].get("persona", "unclassified"),
                        "state": row["_id"]["state"],
                        "carry_in": bool(row["_id"].get("carry_in", False)),
                        "count": row["count"],
                    }
                    async for row in cursor
                ]
                if rollup:
                    await self._update_run({"$set": {f"archived_normal.{day_iso}": rollup}})
            deleted = await self._operations.delete_many(filt)
            return int(deleted.deleted_count)

    async def _archived_normal(self) -> dict[str, list[dict]]:
        run = await self._runs.find_one(self._run_key(), {"archived_normal": 1})
        return (run or {}).get("archived_normal") or {}

    async def totals(self) -> dict:
        """Count operation states and distinct credited identities by window and kind."""
        archived = await self._archived_normal()
        match: dict[str, Any] = {"run_id": self.run_id}
        if archived:
            match["episode_id"] = {"$not": {"$regex": _normal_day_regex(sorted(archived))}}
        cursor = await self._operations.aggregate(
            [
                {"$match": match},
                {"$group": {"_id": "$state", "count": {"$sum": 1}}},
            ]
        )
        states = Counter({row["_id"]: row["count"] async for row in cursor})
        for rows in archived.values():
            for row in rows:
                states[row["state"]] += row["count"]
        credit: dict[str, Counter] = {}
        async for row in self._credit.find({"run_id": self.run_id}, {"window": 1, "kind": 1}):
            credit.setdefault(row["window"], Counter())[row["kind"]] += 1
        return {"states": dict(states), "credit": {window: dict(counts) for window, counts in credit.items()}}

    async def scenario_outcomes(self) -> tuple[dict[tuple[str, str], set[str]], set[tuple[str, str, str]]]:
        cursor = await self._operations.aggregate(
            [
                {"$match": {"run_id": self.run_id, "episode_kind": {"$not": {"$regex": "^normal_"}}}},
                {
                    "$group": {
                        "_id": {"episode": "$episode_id", "actor": "$actor"},
                        "states": {"$addToSet": "$state"},
                    }
                },
            ]
        )
        states = {(row["_id"]["episode"], row["_id"]["actor"]): set(row["states"]) async for row in cursor}
        credits = {
            (row["window"], row["kind"], row["identity"])
            async for row in self._credit.find(
                {"run_id": self.run_id}, {"window": 1, "kind": 1, "identity": 1}
            )
        }
        return states, credits

    async def operation_breakdown(self) -> dict:
        archived = await self._archived_normal()
        match: dict[str, Any] = {"run_id": self.run_id}
        if archived:
            match["episode_id"] = {"$not": {"$regex": _normal_day_regex(sorted(archived))}}
        rows = await self._operations.aggregate(
            [
                {"$match": match},
                {
                    "$group": {
                        "_id": {
                            "class": "$scenario_class",
                            "persona": "$persona",
                            "state": "$state",
                            "carry_in": "$prior_to_live",
                        },
                        "count": {"$sum": 1},
                    }
                },
            ]
        )
        breakdown = {}

        def _add(cls: str, persona: str, state: str, carry_in: bool, count: int) -> None:
            counts = breakdown.setdefault(cls, {}).setdefault(
                persona,
                dict.fromkeys(
                    ("prepared", "attempted", "completed", "rejected", "unresolved", "carried_in"), 0
                ),
            )
            if state == "prepared":
                counts["prepared"] += count
                return
            counts["attempted"] += count
            if carry_in:
                counts["carried_in"] += count
            outcome = {"succeeded": "completed", "declined": "rejected"}.get(state, "unresolved")
            counts[outcome] += count

        async for row in rows:
            key = row["_id"]
            _add(
                key.get("class", "unclassified"),
                key.get("persona", "unclassified"),
                key["state"],
                bool(key.get("carry_in")),
                row["count"],
            )
        for day_rows in archived.values():
            for row in day_rows:
                _add(row["class"], row["persona"], row["state"], bool(row.get("carry_in")), row["count"])
        return breakdown

    async def finish(self, summary: dict) -> None:
        _check_safe(summary)
        async with self._lock:
            await self._require_owner()
            if await self.unresolved():
                raise RecoveryError("stage 6 run has unresolved operations")
            await self._update_run({"$set": {"completed": True, "summary": summary}})
