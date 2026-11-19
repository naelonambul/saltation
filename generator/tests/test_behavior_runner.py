import json
from dataclasses import replace
from datetime import date, timedelta

import httpx
import pytest
from test_bc import MemoryTruth

from saltation_gen.client.bank import BankClient
from saltation_gen.drivers import behavior
from saltation_gen.drivers.behavior_plan import FIXTURE_START, SUBJECTS, fixture_plan, instant, validate_plan
from saltation_gen.drivers.seed import make_customers
from saltation_gen.truth.recovery import RecoveryError

DAY = date(2026, 11, 1)
SEED = 20261112


class BehaviorTruth(MemoryTruth):
    async def load_roster(self):
        return [
            row | {"fintech_use_num": f"998{i:021d}"}
            for i, row in enumerate(make_customers(SEED, len(SUBJECTS), FIXTURE_START))
        ]


class Fixture:
    def __init__(self):
        self.truth = BehaviorTruth()
        self.at = instant(DAY, 9, 58)
        self.calls = []
        self.moves = []
        self.mode = "BACKFILL"
        self.lose = None

    async def run(self):
        bank = BankClient("http://bank", transport=httpx.MockTransport(self.handle))
        try:
            return await behavior.run(bank, self.truth, "proof", SEED, DAY)
        finally:
            await bank.close()

    def handle(self, request):
        path = request.url.path
        if path == "/internal/v1/outbox":
            return httpx.Response(200, json={"undelivered": 0})
        if path == "/internal/v1/clock/set":
            self.at = type(self.at).fromisoformat(json.loads(request.content)["business_time"])
            self.moves.append(self.at)
        if path.startswith("/internal/v1/clock"):
            return httpx.Response(200, json={"mode": self.mode, "business_time": self.at.isoformat()})

        rid = request.headers["x-request-id"]
        operations = self.truth.runs.rows["proof"]["operations"]
        ((name, op),) = [(name, op) for name, op in operations.items() if op["request_id"] == rid]
        assert op["state"] == "unconfirmed"
        labels = [row for row in self.truth.labels.rows.values() if row["request_id"] == rid]
        assert labels == op["labels"]
        assert self.at.isoformat() == op["business_time"]
        assert request.headers["x-sim-ip"] == op["context"]["ip"]
        self.calls.append(name)

        if name == self.lose:
            raise httpx.ReadTimeout("response lost")
        if path == behavior.LOGIN:
            assert len(labels) == 1 and labels[0]["subject_type"] == "customer"
            return httpx.Response(200, json={"access_token": "holder-token"})
        assert len(labels) == 2 and all(label["subject_type"] == "account" for label in labels)
        assert labels[1]["label"] == "normal"
        assert request.headers["authorization"] == "Bearer holder-token"
        body = json.loads(request.content)
        assert body["bank_tran_id"] == op["payload"]["transaction_key"]
        assert body["tran_dtime"] == self.at.strftime("%Y%m%d%H%M%S")
        return httpx.Response(200, json={"rsp_code": "A0000", "bank_rsp_code": "000"})


async def test_fixture_records_each_motif_and_reads_completed_run_without_requests():
    fixture = Fixture()
    result = await fixture.run()
    assert result == await fixture.run()
    assert result["attempted"] == result["completed"] == len(fixture.calls) == 18
    assert result["instances"] == {"E": 1, "F": 1, "lookalike": 4}
    assert result["unresolved"] == result["recovery_logins"] == 0
    assert len(fixture.truth.labels.rows) == 24
    assert fixture.truth.runs.rows["proof"]["inputs"]["f_active_hours_kst"] == [8, 22]
    assert "holder-token" not in repr(fixture.truth.runs.rows)
    house_ids = {result["requests"][name] for name in ("household_1", "household_2")}
    house_labels = [row for row in fixture.truth.labels.rows.values() if row["request_id"] in house_ids]
    assert {row["scenario_instance_id"] for row in house_labels} == {"proof:household"}


def test_purchase_inside_velocity_reservation_is_rejected():
    actions, reservations = fixture_plan(DAY)
    velocity = next(row for row in reservations if row.motif == "velocity")
    overlap = velocity.start + timedelta(seconds=5)
    moved = [replace(row, at=overlap) if row.motif == "purchase" else row for row in actions]
    changed = [
        replace(row, start=overlap, end=overlap) if row.motif == "purchase" else row for row in reservations
    ]
    with pytest.raises(ValueError, match="reservations overlap"):
        validate_plan(sorted(moved, key=lambda row: row.at), changed)


async def test_wrong_clock_mode_is_rejected_before_dispatch():
    fixture = Fixture()
    fixture.mode = "LIVE"
    with pytest.raises(RecoveryError, match="requires a bank in BACKFILL"):
        await fixture.run()
    assert not fixture.calls and not fixture.moves


@pytest.mark.parametrize("lose_recovery", [False, True])
async def test_prepared_transfer_uses_separately_labelled_recovery_login(monkeypatch, lose_recovery):
    fixture = Fixture()
    dispatch = behavior._BehaviorRun.dispatch

    async def interrupted(proof, action):
        if action.name == "e_1":
            raise RecoveryError("interrupted after transfer prepare")
        await dispatch(proof, action)

    with monkeypatch.context() as patch:
        patch.setattr(behavior._BehaviorRun, "dispatch", interrupted)
        with pytest.raises(RecoveryError, match="after transfer prepare"):
            await fixture.run()
    original = fixture.truth.runs.rows["proof"]["operations"]["e_1"]
    assert original["state"] == "prepared"
    recovery = "e_1_recovery_1"
    if lose_recovery:
        fixture.lose = recovery
        with pytest.raises(RecoveryError, match=f"{recovery} is unconfirmed"):
            await fixture.run()
        calls, moves = fixture.calls.copy(), fixture.moves.copy()
        with pytest.raises(RecoveryError, match="requires diagnosis"):
            await fixture.run()
        assert fixture.calls == calls and fixture.moves == moves
        assert "e_1" not in fixture.calls
        return

    result = await fixture.run()
    assert result["recovery_logins"] == 1
    assert result["requests"]["e_1"] == original["request_id"]
    recovery_id = result["requests"][recovery]
    (label,) = [row for row in fixture.truth.labels.rows.values() if row["request_id"] == recovery_id]
    assert label["subject_type"] == "customer"
    assert (label["label"], label["scenario_instance_id"]) == ("E", "proof:velocity")
    position = fixture.calls.index(recovery)
    assert fixture.calls[position + 1] == "e_1"


async def test_unconfirmed_transfer_stops_without_replay_or_clock_move():
    fixture = Fixture()
    fixture.lose = "e_1"
    with pytest.raises(RecoveryError, match="e_1 is unconfirmed"):
        await fixture.run()
    prior_calls, prior_moves = fixture.calls.copy(), fixture.moves.copy()
    with pytest.raises(RecoveryError, match="requires diagnosis"):
        await fixture.run()
    assert fixture.calls == prior_calls and fixture.moves == prior_moves


def test_time_anomaly_cannot_be_scheduled_inside_f_active_hours():
    actions, reservations = fixture_plan(DAY)
    anomaly = next(item for item in actions if item.name == "f_anomaly")
    wrong = anomaly.at.replace(hour=8)
    changed = [replace(item, at=wrong) if item == anomaly else item for item in actions]
    updated = [
        replace(item, start=wrong, end=wrong) if item.motif == "late_hour" else item for item in reservations
    ]
    with pytest.raises(ValueError, match="active hours"):
        validate_plan(sorted(changed, key=lambda item: item.at), updated)
