"""Offline Mongo fakes for the stage 6 journal's single-document writes."""

import asyncio
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
        if actual != expected:
            return False
    return True


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

    async def aggregate(self, pipeline):
        rows = list(self.rows)
        for stage in pipeline:
            if "$match" in stage:
                rows = [r for r in rows if matches(r, stage["$match"])]
            elif "$group" in stage:
                field = stage["$group"]["_id"].lstrip("$")
                counts = {}
                for r in rows:
                    key = r.get(field)
                    counts[key] = counts.get(key, 0) + 1
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
                row.update(deepcopy(update.get("$set", {})))
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
