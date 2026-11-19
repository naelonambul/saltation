"""Exercise recovery state and pre-dispatch labels without a live bank."""

from copy import deepcopy
from types import SimpleNamespace

import httpx
import pytest
from pymongo.errors import DuplicateKeyError

from saltation_gen.client.bank import BankClient
from saltation_gen.drivers import bc
from saltation_gen.truth.recovery import RecoveryError, RecoveryJournal

CLOCK = {"mode": "BACKFILL", "business_time": "2026-11-01T12:00:00+09:00"}
TRANSFER = "/v2.0/transfer/withdraw/fin_num"
WITHDRAW = "/atm/v1/cash/withdraw"
ACCOUNTS = ("99800000001", "99800000002")


class MemoryCollection:
    def __init__(self):
        self.rows = {}
        self.interrupt_receiver_label = False

    def with_options(self, *, write_concern):
        assert write_concern.document == {"j": True, "w": 1}
        return self

    async def insert_one(self, doc):
        key = doc["_id"]
        if key in self.rows:
            raise DuplicateKeyError("duplicate")
        self.rows[key] = deepcopy(doc)

    async def find_one_and_update(self, query, update, *, return_document):
        assert return_document
        doc = self.rows.get(query["_id"])
        if doc is None or any(doc.get(key) != value for key, value in query.items()):
            return None
        doc.update(update["$set"])
        return deepcopy(doc)

    async def update_one(self, query, update, *, upsert=False):
        if upsert:
            value = update["$setOnInsert"]
            key = (query["request_id"], query["subject_id"])
            if self.interrupt_receiver_label and query["subject_id"] == ACCOUNTS[1]:
                raise RuntimeError("label insert interrupted")
            self.rows.setdefault(key, deepcopy(value))
            return SimpleNamespace(matched_count=1)
        doc = self.rows.get(query["_id"], {})
        if doc.get("owner") != query["owner"]:
            return SimpleNamespace(matched_count=0)
        for path, expected in query.items():
            actual = doc
            for part in path.split("."):
                actual = actual.get(part) if isinstance(actual, dict) else None
            if expected == {"$exists": False}:
                assert actual is None
            elif actual != expected:
                return SimpleNamespace(matched_count=0)
        for path, value in update.get("$set", {}).items():
            target = doc
            *parents, field = path.split(".")
            for parent in parents:
                target = target[parent]
            target[field] = deepcopy(value)
        for field in update.get("$unset", {}):
            doc.pop(field)
        return SimpleNamespace(matched_count=1)

    async def find(self, query, projection):
        assert projection == {"_id": 0}
        for row in self.rows.values():
            if row["request_id"] == query["request_id"]:
                yield deepcopy(row)


class MemoryTruth:
    def __init__(self):
        self.labels = MemoryCollection()
        self.runs = MemoryCollection()

    async def load_roster(self):
        return [
            {
                "customer_no": f"C{i:08d}",
                "account_no": account,
                "phone": f"010{i:08d}",
                "password": "fixture-password",
                "atm_pin": "1234",
                "fintech_use_num": str(i),
                "name": f"Synthetic {i}",
            }
            for i, account in enumerate(ACCOUNTS, 1)
        ]


class Fixture:
    def __init__(self, truth, *, lost=None, reject=None):
        self.truth = truth
        self.lost = lost
        self.reject = reject
        self.sent = []
        self.clock = CLOCK

    async def run(self):
        bank = BankClient("http://bank", transport=httpx.MockTransport(self.handle))
        try:
            return await bc.run(bank, self.truth, "run", *ACCOUNTS, 50_000, 10_000)
        finally:
            await bank.close()

    def handle(self, request):
        path = request.url.path
        if path == "/internal/v1/clock":
            return httpx.Response(200, json=self.clock)
        rid = request.headers["x-request-id"]
        labels = [doc for doc in self.truth.labels.rows.values() if doc["request_id"] == rid]
        assert len(labels) == (2 if path == TRANSFER else 1)
        self.sent.append((path, rid, request.content))
        if self.lost == path:
            raise httpx.ReadTimeout("simulated loss")
        return httpx.Response(
            self.reject if self.reject and path == TRANSFER else 200,
            json={
                "rsp_code": "A0000",
                "bank_rsp_code": "000",
                "access_token": "fixture-bearer",
                "atm_session_token": "fixture-atm",
            },
        )


async def test_partial_transfer_labels_are_completed_before_dispatch():
    truth = MemoryTruth()
    truth.labels.interrupt_receiver_label = True
    fixture = Fixture(truth)
    with pytest.raises(RuntimeError, match="label insert interrupted"):
        await fixture.run()
    assert [path for path, _, _ in fixture.sent] == ["/v2.0/ext/auth/login"]
    transfer = truth.runs.rows["run"]["operations"]["transfer"]
    assert transfer["state"] == "prepared"
    assert (transfer["request_id"], ACCOUNTS[0]) in truth.labels.rows

    truth.labels.interrupt_receiver_label = False
    await fixture.run()
    assert sum(path == TRANSFER for path, _, _ in fixture.sent) == 1
    assert (transfer["request_id"], ACCOUNTS[1]) in truth.labels.rows


async def test_conflicting_label_prevents_transfer():
    truth = MemoryTruth()
    truth.labels.interrupt_receiver_label = True
    fixture = Fixture(truth)
    with pytest.raises(RuntimeError, match="label insert interrupted"):
        await fixture.run()
    truth.labels.interrupt_receiver_label = False
    transfer = truth.runs.rows["run"]["operations"]["transfer"]
    truth.labels.rows[transfer["request_id"], ACCOUNTS[0]]["scenario_instance_id"] = "conflict"
    with pytest.raises(RecoveryError, match="conflicting or incomplete labels"):
        await fixture.run()
    assert all(path != TRANSFER for path, _, _ in fixture.sent)


async def test_lost_transfer_replays_exact_request_and_completed_runs_send_nothing():
    truth = MemoryTruth()
    first = Fixture(truth, lost=TRANSFER)
    with pytest.raises(RecoveryError, match="transfer outcome is unresolved"):
        await first.run()
    second = Fixture(truth)
    result = await second.run()
    assert first.sent[-1] == second.sent[1]
    assert first.sent[0][1] != second.sent[0][1]
    assert second.sent[-1][0] == WITHDRAW
    assert await second.run() == result
    assert len(second.sent) == 4
    saved = repr(truth.runs.rows)
    assert "fixture-password" not in saved
    assert "fixture-bearer" not in saved
    assert "fixture-atm" not in saved


@pytest.mark.parametrize("status", [401, 409, 422])
async def test_rejection_after_ambiguous_transfer_leaves_it_unresolved(status):
    truth = MemoryTruth()
    with pytest.raises(RecoveryError, match="transfer outcome is unresolved"):
        await Fixture(truth, lost=TRANSFER).run()
    replay = Fixture(truth, reject=status)
    with pytest.raises(RecoveryError, match="transfer outcome is unresolved"):
        await replay.run()
    assert truth.runs.rows["run"]["operations"]["transfer"]["state"] == "unconfirmed"
    assert len(replay.sent) == 2


async def test_ambiguous_withdrawal_cannot_be_reissued():
    truth = MemoryTruth()
    with pytest.raises(RecoveryError, match="ATM withdrawal outcome is unresolved"):
        await Fixture(truth, lost=WITHDRAW).run()
    next_run = Fixture(truth)
    with pytest.raises(RecoveryError, match="ATM state requires diagnosis"):
        await next_run.run()
    assert not next_run.sent


async def test_claim_is_exclusive_and_stale_owner_must_match():
    truth = MemoryTruth()
    first = RecoveryJournal(truth, "run", "first")
    second = RecoveryJournal(truth, "run", "second")
    await first.claim({}, None)
    for proposed_owner in (None, "other"):
        with pytest.raises(RecoveryError, match="already has an owner"):
            await second.claim({}, proposed_owner)
    await second.claim({}, "first")
    with pytest.raises(RecoveryError, match="owner or operation state changed"):
        await first.release()
    await second.release()


async def test_restart_after_confirmed_transfer_requires_the_same_frozen_time():
    truth = MemoryTruth()
    fixture = Fixture(truth)
    # Allow both transfer labels, then interrupt before the ATM session start.
    update = truth.labels.update_one

    async def interrupt_start(query, mutation, *, upsert=False):
        if (
            upsert
            and query["subject_id"] == ACCOUNTS[1]
            and any(path == TRANSFER for path, _, _ in fixture.sent)
        ):
            raise RuntimeError("interrupted before ATM")
        return await update(query, mutation, upsert=upsert)

    truth.labels.update_one = interrupt_start
    with pytest.raises(RuntimeError, match="interrupted before ATM"):
        await fixture.run()
    assert truth.runs.rows["run"]["operations"]["transfer"]["state"] == "succeeded"

    resumed = Fixture(truth)
    resumed.clock = {**CLOCK, "business_time": "2026-11-01T12:01:00+09:00"}
    with pytest.raises(RecoveryError, match="business time changed before ATM dispatch"):
        await resumed.run()
    assert not resumed.sent


async def test_clock_change_after_atm_session_prevents_withdrawal():
    truth = MemoryTruth()
    fixture = Fixture(truth)
    original = fixture.handle

    def handle(request):
        response = original(request)
        if request.url.path == "/atm/v1/session":
            fixture.clock = {**CLOCK, "business_time": "2026-11-01T12:01:00+09:00"}
        return response

    fixture.handle = handle
    with pytest.raises(RecoveryError, match="business time changed before withdrawal dispatch"):
        await fixture.run()
    assert fixture.sent[-1][0] == "/atm/v1/session"
    assert truth.runs.rows["run"]["operations"]["withdraw"]["state"] == "prepared"
