"""Offline Mongo fakes for the stage 6 journal's single-document writes."""

import asyncio
import re
from copy import deepcopy
from types import SimpleNamespace

import pytest
from pymongo import ReturnDocument
from pymongo.errors import BulkWriteError, DuplicateKeyError

from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal
from saltation_gen.truth.stage6 import Stage6Journal


class InjectedExit(Exception):
    pass


def matches(row, query):
    for key, expected in query.items():
        actual = row.get(key)
        if isinstance(expected, dict) and "$eq" in expected:
            expected = expected["$eq"]
        elif isinstance(expected, dict) and "$in" in expected:
            if actual not in expected["$in"]:
                return False
            continue
        elif isinstance(expected, dict) and "$nin" in expected:
            if actual in expected["$nin"]:
                return False
            continue
        elif isinstance(expected, dict) and "$regex" in expected:
            if not isinstance(actual, str) or re.search(expected["$regex"], actual) is None:
                return False
            continue
        elif isinstance(expected, dict) and "$not" in expected and "$regex" in expected["$not"]:
            if isinstance(actual, str) and re.search(expected["$not"]["$regex"], actual) is not None:
                return False
            continue
        if actual != expected:
            return False
    return True


def _apply_set(row, fields):
    for key, value in fields.items():
        if "." in key:
            head, tail = key.split(".", 1)
            target = row.setdefault(head, {})
            target[tail] = deepcopy(value)
        else:
            row[key] = deepcopy(value)


class Collection:
    def __init__(self, database):
        self.database = database
        self.rows = []
        self.write_concern = None
        self.exit_before_claim = False
        self.exit_after_upsert = False
        self.exit_before_delete = False
        self.change_before_update = None
        self.indexes = []

    def with_options(self, *, write_concern):
        self.write_concern = write_concern
        return self

    def durable(self):
        assert self.write_concern.document == {"w": 1, "j": True}

    async def create_index(self, keys, *, name=None):
        self.durable()
        self.indexes.append((keys, name))

    async def insert_one(self, document):
        self.durable()
        if any(row["_id"] == document["_id"] for row in self.rows):
            raise DuplicateKeyError("duplicate _id")
        self.rows.append(deepcopy(document))

    async def insert_many(self, documents, *, ordered=True):
        self.durable()
        write_errors = []
        for idx, document in enumerate(documents):
            if "_id" not in document and {"request_id", "subject_type", "subject_id"} <= document.keys():
                # Mirror real PyMongo insert_many in-place _id assignment.
                document["_id"] = (document["request_id"], document["subject_type"], document["subject_id"])
            doc = deepcopy(document)
            if any(row["_id"] == doc["_id"] for row in self.rows):
                write_errors.append({"index": idx, "code": 11000, "errmsg": "duplicate _id"})
                if ordered:
                    break
                continue
            self.rows.append(doc)
        if write_errors:
            raise BulkWriteError({"writeErrors": write_errors, "writeConcernErrors": []})

    async def update_many(self, query, update):
        self.durable()
        matched = 0
        for row in self.rows:
            if matches(row, query):
                row.update(deepcopy(update.get("$set", {})))
                for key in update.get("$unset", {}):
                    row.pop(key, None)
                matched += 1
        return SimpleNamespace(matched_count=matched)

    async def bulk_write(self, requests, *, ordered=True):
        self.durable()
        matched = 0
        for req in requests:
            res = await self.update_one(req._filter, req._doc, upsert=req._upsert)
            matched += res.matched_count
        return SimpleNamespace(matched_count=matched)

    async def delete_many(self, query):
        self.durable()
        before = len(self.rows)
        self.rows = [row for row in self.rows if not matches(row, query)]
        return SimpleNamespace(deleted_count=before - len(self.rows))

    async def aggregate(self, pipeline):
        rows = list(self.rows)
        for stage in pipeline:
            if "$match" in stage:
                rows = [r for r in rows if matches(r, stage["$match"])]
            elif "$group" in stage:
                id_spec = stage["$group"]["_id"]
                counts = {}
                for r in rows:
                    if isinstance(id_spec, dict):
                        key = tuple((k, r.get(v.lstrip("$"))) for k, v in id_spec.items())
                    else:
                        key = r.get(id_spec.lstrip("$"))
                    counts[key] = counts.get(key, 0) + 1
                if isinstance(id_spec, dict):
                    rows = [{"_id": dict(k), "count": v} for k, v in counts.items()]
                else:
                    rows = [{"_id": k, "count": v} for k, v in counts.items()]

        async def _iter():
            for row in rows:
                yield deepcopy(row)

        return _iter()

    async def find_one(self, query, projection=None):
        await asyncio.sleep(0)
        return await anext(self.find(query, projection), None)

    async def find(self, query, projection=None):
        for row in self.rows:
            if matches(row, query):
                if projection is None:
                    yield deepcopy(row)
                elif all(v == 0 for v in projection.values()):
                    yield deepcopy({k: v for k, v in row.items() if k not in projection})
                else:
                    yield deepcopy({k: v for k, v in row.items() if projection.get(k)})

    async def update_one(self, query, update, *, upsert=False):
        self.durable()
        if self.change_before_update is not None:
            self.change_before_update(self.rows)
            self.change_before_update = None
        for row in self.rows:
            if matches(row, query):
                _apply_set(row, update.get("$set", {}))
                for key in update.get("$unset", {}):
                    row.pop(key, None)
                return SimpleNamespace(matched_count=1)
        if upsert:
            await self.insert_one(query | update["$setOnInsert"])
            if self.exit_after_upsert:
                self.exit_after_upsert = False
                raise InjectedExit
        return SimpleNamespace(matched_count=0)

    async def find_one_and_update(self, query, update, *, return_document):
        assert return_document == ReturnDocument.AFTER
        if self.exit_before_claim:
            self.exit_before_claim = False
            raise InjectedExit
        result = await self.update_one(query, update)
        if result.matched_count == 0:
            return None
        return await self.find_one({"_id": query["_id"], **update["$set"]})

    async def delete_one(self, query):
        self.durable()
        if self.exit_before_delete:
            self.exit_before_delete = False
            raise InjectedExit
        for i, row in enumerate(self.rows):
            if matches(row, query):
                self.rows.pop(i)
                return SimpleNamespace(deleted_count=1)
        return SimpleNamespace(deleted_count=0)


class Database(dict):
    def __missing__(self, name):
        self[name] = Collection(self)
        return self[name]


@pytest.fixture
def truth():
    return SimpleNamespace(runs=Database()["runs"])


@pytest.fixture
def backfill_inputs():
    return {
        "version": 2,
        "workload_version": 2,
        "seed": 7,
        "config": {"population": 20},
        "run": {"first": "2026-01-01", "days": 3, "live_at": None},
        "backfill_end": "2026-01-04T00:00:00+09:00",
        "schedule_digest": "sha256:backfill",
        "backfill_schedule_digest": "sha256:backfill",
        "planned_counts": {
            "backfill": {"episodes": {"A": 1}, "operations": 10},
            "live": {"episodes": {}, "operations": 0},
        },
    }


async def test_claim_validates_provenance_before_writing(truth):
    journal = Stage6Journal(truth, "run", "owner")
    with pytest.raises(RecoveryError, match="credential fields"):
        await journal.claim({}, provenance={"nested": {"access_token": "x"}})
    assert truth.runs.rows == []
    assert truth.runs.database["stage6_leases"].rows == []


async def test_bank_and_run_ownership_and_stale_claim(truth):
    first = Stage6Journal(truth, "run", "first")
    competitor = Stage6Journal(truth, "other", "competitor")
    resumed = Stage6Journal(truth, "run", "resumed")
    assert (await first.claim({"seed": 1}))["owner"] == "first"
    await first.save_progress({"current": "timestamp"})
    with pytest.raises(RecoveryError, match="bank lease"):
        await competitor.claim({"seed": 1})
    with pytest.raises(RecoveryError, match="bank lease"):
        await competitor.claim({"seed": 1}, exited_owner="first")
    with pytest.raises(RecoveryError, match="bank lease"):
        await resumed.claim({"seed": 1}, exited_owner="wrong")
    with pytest.raises(RecoveryError, match="inputs differ"):
        await resumed.claim({"seed": 2}, exited_owner="first")

    assert (await resumed.claim({"seed": 1}, exited_owner="first"))["progress"] == {"current": "timestamp"}
    with pytest.raises(RecoveryError, match="lease is no longer held"):
        await first.save_progress({})
    with pytest.raises(RecoveryError, match="lease is no longer held"):
        await first.release()
    await resumed.release()
    assert (await competitor.claim({"seed": 1}))["owner"] == "competitor"


async def test_competing_fresh_claims_have_one_winner(truth):
    journals = [Stage6Journal(truth, "run", owner) for owner in ("first", "second")]
    outcomes = await asyncio.gather(*(journal.claim({}) for journal in journals), return_exceptions=True)
    assert sum(isinstance(outcome, dict) for outcome in outcomes) == 1
    assert sum(isinstance(outcome, RecoveryError) for outcome in outcomes) == 1


async def test_recover_exit_between_lease_and_run_claim(truth):
    first = Stage6Journal(truth, "run", "first")
    interrupted = Stage6Journal(truth, "run", "interrupted")
    resumed = Stage6Journal(truth, "run", "resumed")
    await first.claim({})
    truth.runs.exit_before_claim = True
    with pytest.raises(InjectedExit):
        await interrupted.claim({}, exited_owner="first")
    with pytest.raises(RecoveryError, match="bank lease"):
        await resumed.claim({}, exited_owner="first")
    assert (await resumed.claim({}, exited_owner="interrupted"))["owner"] == "resumed"
    await resumed.release()


async def test_recover_exit_between_run_and_lease_release(truth):
    first = Stage6Journal(truth, "run", "first")
    other = Stage6Journal(truth, "other", "other")
    await first.claim({})
    truth.runs.database["stage6_leases"].exit_before_delete = True
    with pytest.raises(InjectedExit):
        await first.release()
    with pytest.raises(RecoveryError, match="bank lease"):
        await other.claim({})
    resumed = Stage6Journal(truth, "run", "resumed")
    await resumed.claim({}, exited_owner="first")
    await resumed.release()
    await other.claim({})


async def test_prepare_is_idempotent_and_keys_do_not_collide(truth):
    journal = Stage6Journal(truth, "run", "owner")
    await journal.claim({})
    original = {
        "request_id": "rid",
        "request_key": "key",
        "labels": [{"request_id": "rid", "label": "A"}],
    }
    row = await journal.prepare("a:b", "c", original)
    retry = {
        "request_id": "fresh",
        "request_key": "fresh-key",
        "labels": [{"request_id": "fresh", "label": "A"}],
    }
    assert await journal.prepare("a:b", "c", retry) == row
    with pytest.raises(RecoveryError, match="differs"):
        await journal.prepare("a:b", "c", original | {"amount": "10"})
    other = await journal.prepare("a", "b:c", retry)
    assert other["_id"] != row["_id"]
    assert (await journal.get_operation("a:b", "c"))["request_id"] == "rid"
    assert await Stage6Journal(truth, "other", "reader").get_operation("a:b", "c") is None


async def test_competing_transitions_and_exact_row_owner_fence(truth):
    first = Stage6Journal(truth, "run", "first")
    await first.claim({})
    await first.prepare("e", "send", {"request_id": "rid"})
    resumed = Stage6Journal(truth, "run", "resumed")
    await resumed.claim({}, exited_owner="first")
    with pytest.raises(RecoveryError, match="lease is no longer held"):
        await first.transition("e", "send", "prepared", "unconfirmed")

    operations = truth.runs.database["stage6_operations"]
    operations.change_before_update = lambda rows: rows[0].update(owner="different")
    with pytest.raises(RecoveryError, match="operation owner or state"):
        await resumed.transition("e", "send", "prepared", "unconfirmed")
    assert (await resumed.get_operation("e", "send"))["state"] == "prepared"

    results = await asyncio.gather(
        *(resumed.transition("e", "send", "prepared", "unconfirmed") for _ in range(2)),
        return_exceptions=True,
    )
    assert results.count(None) == 1
    assert sum(isinstance(result, RecoveryError) for result in results) == 1
    assert (await resumed.get_operation("e", "send"))["owner"] == "resumed"
    assert await resumed.unresolved()
    with pytest.raises(RecoveryError, match="unresolved"):
        await resumed.finish({})
    with pytest.raises(RecoveryError, match="credential"):
        await resumed.transition("e", "send", "unconfirmed", "succeeded", {"nested": {"access_token": "x"}})
    await resumed.transition("e", "send", "unconfirmed", "succeeded", {"status": 200})
    assert (await resumed.get_operation("e", "send"))["result"] == {"status": 200}
    assert not await resumed.unresolved()


async def test_credit_recovers_after_committed_write_and_resets_across_windows(truth):
    first = Stage6Journal(truth, "run", "first")
    await first.claim({})
    await first.prepare("e", "send", {"request_id": "rid"})
    with pytest.raises(RecoveryError, match="terminal operation"):
        await first.credit("jan", "class", "actor", "e")
    await first.transition("e", "send", "prepared", "unconfirmed")
    await first.transition("e", "send", "unconfirmed", "succeeded")
    truth.runs.database["stage6_credit"].exit_after_upsert = True
    with pytest.raises(InjectedExit):
        await first.credit("jan", "class", "actor", "e")

    resumed = Stage6Journal(truth, "run", "resumed")
    await resumed.claim({}, exited_owner="first")
    await resumed.credit("jan", "class", "actor", "e")
    await resumed.credit("feb", "class", "actor", "e")
    expected = {"states": {"succeeded": 1}, "credit": {"jan": {"class": 1}, "feb": {"class": 1}}}
    assert await resumed.totals() == expected
    await resumed.finish(expected)
    await resumed.release()
    assert await Stage6Journal(truth, "other", "reader").totals() == {"states": {}, "credit": {}}
    stored = await truth.runs.find_one({"_id": "run"})
    assert stored["completed"] is True
    assert stored["summary"] == expected
    assert "operations" not in stored
    assert "owner" not in stored


async def test_batch_prepare_labels_unconfirmed_and_complete_with_resume(truth):
    truth.labels = truth.runs.database["labels"]
    first = Stage6Journal(truth, "run", "first")
    await first.claim({})
    prepared = await first.prepare_batch(
        [
            ("e1", "step1", {"request_id": "rid1", "args": {"a": 1}}),
            ("e2", "step2", {"request_id": "rid2", "args": {"a": 2}}),
        ]
    )
    assert set(prepared) == {("e1", "step1"), ("e2", "step2")}

    labels = RecoveryJournal(truth, "run", "first")
    expected_labels = {
        "rid1": [{"request_id": "rid1", "subject_type": "account", "subject_id": "a1"}],
        "rid2": [{"request_id": "rid2", "subject_type": "account", "subject_id": "a2"}],
    }
    await labels.ensure_labels_batch(expected_labels)
    assert all("_id" not in label for items in expected_labels.values() for label in items)
    # Re-running on resume succeeds idempotently via BulkWriteError 11000 readback.
    await labels.ensure_labels_batch(expected_labels)

    # Mark wave unconfirmed and complete e1 only (simulate partial-wave crash before e2 completed).
    await first.mark_unconfirmed_wave([prepared[("e1", "step1")]])
    await first.complete_wave([("e1", "step1", "succeeded", {"decision": "approved"})])
    assert await first.totals() == {"states": {"succeeded": 1, "prepared": 1}, "credit": {}}

    # Simulate takeover and resume of the same chunk: e1 stays succeeded, e2 stays prepared.
    resumed = Stage6Journal(truth, "run", "resumed")
    await resumed.claim({}, exited_owner="first")
    reloaded = await resumed.prepare_batch(
        [
            ("e1", "step1", {"request_id": "new-rid1", "args": {"a": 1}}),
            ("e2", "step2", {"request_id": "new-rid2", "args": {"a": 2}}),
        ]
    )
    assert reloaded[("e1", "step1")]["state"] == "succeeded"
    assert reloaded[("e2", "step2")]["request_id"] == "rid2"
    assert reloaded[("e2", "step2")]["owner"] == "first"

    # Adopting e2 from prior owner transitions cleanly to unconfirmed then succeeded.
    await resumed.mark_unconfirmed_wave([reloaded[("e2", "step2")]])
    await resumed.complete_wave([("e2", "step2", "declined", {"decision": "rejected"})])
    assert await resumed.totals() == {"states": {"succeeded": 1, "declined": 1}, "credit": {}}

    # Stale owner is fenced out of mark_unconfirmed_wave.
    with pytest.raises(RecoveryError, match="lease is no longer held"):
        await first.mark_unconfirmed_wave([reloaded[("e2", "step2")]])


async def test_prune_completed_normal_day_preserves_totals_and_is_idempotent(truth, backfill_inputs):
    journal = Stage6Journal(truth, "run", "owner-1")
    await journal.claim(backfill_inputs)
    prepared = await journal.prepare_batch(
        [
            (
                "normal_transfer:2026-01-01:C1",
                "transfer",
                {"scenario_class": "normal", "persona": "working", "prior_to_live": False},
            ),
            (
                "normal_card:2026-01-01:C2",
                "card",
                {"scenario_class": "normal", "persona": "student", "prior_to_live": False},
            ),
            (
                "A:2026-01-01:C3",
                "transfer",
                {"scenario_class": "A", "persona": "working", "prior_to_live": False},
            ),
        ]
    )
    await journal.mark_unconfirmed_wave(list(prepared.values()))
    await journal.complete_wave(
        [
            ("normal_transfer:2026-01-01:C1", "transfer", "succeeded", {"status": 200}),
            ("normal_card:2026-01-01:C2", "card", "declined", {"status": 200}),
            ("A:2026-01-01:C3", "transfer", "succeeded", {"status": 200}),
        ]
    )
    before_totals = await journal.totals()
    before_breakdown = await journal.operation_breakdown()

    deleted = await journal.prune_completed_normal_day("2026-01-01")
    assert deleted == 2
    # Scenario A row remains intact in stage6_operations.
    assert len(truth.runs.database["stage6_operations"].rows) == 1
    assert await journal.totals() == before_totals
    assert await journal.operation_breakdown() == before_breakdown

    # Re-running prune (e.g. after crash/resume) is idempotent and never double-counts.
    assert await journal.prune_completed_normal_day("2026-01-01") == 0
    assert await journal.totals() == before_totals
    assert await journal.operation_breakdown() == before_breakdown
    await journal.release()
    resumed = Stage6Journal(truth, "run", "owner-2")
    assert (await resumed.claim(backfill_inputs))["inputs"] == backfill_inputs
    assert await resumed.totals() == before_totals


@pytest.mark.parametrize(
    "mismatch",
    [
        None,
        "seed",
        "version",
        "workload_version",
        "backfill_schedule_digest",
        "backfill_counts",
        "legacy",
        "non_integer_workload_version",
        "non_string_backfill_digest",
        "missing_backfill_counts",
        "malformed_planned_counts",
        "backfill_finished",
        "live_started",
        "live_intended",
        "same_live_at",
        "early_live_at",
    ],
)
async def test_claim_only_attaches_live_before_live_starts_with_matching_backfill(
    truth, backfill_inputs, mismatch
):
    if mismatch in {"live_intended", "same_live_at"}:
        backfill_inputs["run"]["live_at"] = "2026-01-06T10:00:00+09:00"
    if mismatch == "legacy":
        backfill_inputs["version"] = 1
    elif mismatch == "non_integer_workload_version":
        backfill_inputs["workload_version"] = "2"
    elif mismatch == "non_string_backfill_digest":
        backfill_inputs["backfill_schedule_digest"] = None
    elif mismatch == "missing_backfill_counts":
        del backfill_inputs["planned_counts"]["backfill"]
    elif mismatch == "malformed_planned_counts":
        backfill_inputs["planned_counts"] = None
    first = Stage6Journal(truth, "run", "owner-1")
    await first.claim(backfill_inputs)
    await first.save_progress(
        {
            "backfill_finished": mismatch != "backfill_finished",
            "live_started": "2026-01-06T10:00:00+09:00" if mismatch == "live_started" else None,
            "live_intended": mismatch == "live_intended",
        }
    )
    await first.finish({"status": "completed"})
    await first.release()

    live_inputs = deepcopy(backfill_inputs)
    live_inputs["run"]["live_at"] = "2026-01-06T10:00:00+09:00"
    live_inputs["schedule_digest"] = "sha256:backfill-and-live"
    if isinstance(live_inputs["planned_counts"], dict):
        live_inputs["planned_counts"]["live"] = {"episodes": {"F": 1}, "operations": 7}
    if mismatch in {"seed", "version", "workload_version"}:
        live_inputs[mismatch] += 1
    elif mismatch == "backfill_schedule_digest":
        live_inputs[mismatch] = "sha256:different"
    elif mismatch == "backfill_counts":
        live_inputs["planned_counts"]["backfill"]["operations"] += 1
    elif mismatch == "early_live_at":
        live_inputs["run"]["live_at"] = "2026-01-03T10:00:00+09:00"
    elif mismatch == "live_intended":
        live_inputs["run"]["live_at"] = "2026-01-07T10:00:00+09:00"

    second = Stage6Journal(truth, "run", "owner-2")
    if mismatch is not None:
        with pytest.raises(RecoveryError, match="inputs differ") as error:
            await second.claim(live_inputs)
        if mismatch == "backfill_schedule_digest":
            assert "backfill_schedule_digest (stored='sha256:backfill', computed='sha256:different')" in str(
                error.value
            )
        assert (await truth.runs.find_one({"_id": "run"}))["inputs"] == backfill_inputs
        assert truth.runs.database["stage6_leases"].rows == []
        return

    claimed = await second.claim(live_inputs)
    assert claimed["inputs"] == live_inputs
    assert not claimed.get("completed")


async def test_require_success_validates_expected_declined_steps(truth):
    journal = Stage6Journal(truth, "run", "owner-1")
    await journal.claim({"seed": 1})
    prepared = await journal.prepare_batch(
        [
            ("G:1", "small_card", {"scenario_class": "G", "persona": "working"}),
            ("G:1", "declined_card", {"scenario_class": "G", "persona": "working"}),
        ]
    )
    await journal.mark_unconfirmed_wave(list(prepared.values()))
    await journal.complete_wave(
        [
            ("G:1", "small_card", "succeeded", {"decision": "approved"}),
            ("G:1", "declined_card", "declined", {"decision": "rejected"}),
        ]
    )
    await journal.require_success("G:1", ["small_card", "declined_card"], expected_declined=["declined_card"])
    with pytest.raises(RecoveryError, match="every scheduled operation"):
        await journal.require_success("G:1", ["small_card", "declined_card"])
