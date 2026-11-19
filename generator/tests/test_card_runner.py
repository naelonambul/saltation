import json
from dataclasses import replace
from datetime import date, timedelta
from random import Random

import httpx
import pytest
from test_bc import MemoryTruth

from saltation_gen.client.bank import BankClient
from saltation_gen.config.card import (
    CARD_KRW_ANCHORS,
    CARD_KRW_TOLERANCES,
    card_amount_krw,
    sample_domestic_amount,
)
from saltation_gen.drivers import backfill_clock, cards
from saltation_gen.drivers.card_plan import enforce_normal_card_cap, fixture_plan, proof_time
from saltation_gen.drivers.seed import CARD_FIXTURE_START, make_customers
from saltation_gen.truth.recovery import RecoveryError

DAY = date(2026, 11, 1)
SEED = 20261112
CARD_PATH = "/card/v1/authorize"


class CardTruth(MemoryTruth):
    def __init__(self):
        super().__init__()
        self.card_networks = self
        self.people = [
            customer | {"card_no": f"0000{index:012d}"}
            for index, customer in enumerate(make_customers(SEED, 4, CARD_FIXTURE_START), 1)
        ]

    async def find_one(self, query):
        assert query == {"_id": "card-network"}
        return {"client_secret": "fixture-network-secret"}

    async def load_roster(self):
        return self.people


class Fixture:
    def __init__(self):
        self.truth = CardTruth()
        self.at = proof_time(DAY, 8, 59)
        self.moves = []
        self.calls = []
        self.lose_clock_ack = False
        self.lose_backlog_ack = False
        self.lose_card_ack = False
        self.backlog = 0

    async def run(self):
        bank = BankClient("http://bank", transport=httpx.MockTransport(self.handle))
        try:
            return await cards.run(bank, self.truth, "proof", SEED, DAY)
        finally:
            await bank.close()

    def handle(self, request):
        path = request.url.path
        if path == "/internal/v1/outbox":
            if self.lose_backlog_ack:
                self.lose_backlog_ack = False
                raise httpx.ReadTimeout("backlog observation ended after receiving the sample")
            return httpx.Response(200, json={"undelivered": self.backlog})
        if path == "/internal/v1/clock/set":
            previous = self.at
            target = json.loads(request.content)["business_time"]
            progress = self.truth.runs.rows["proof"]["progress"]
            assert progress["intended"] == target
            assert progress["current"] == previous.isoformat()
            self.at = type(self.at).fromisoformat(target)
            self.moves.append(target)
            if self.lose_clock_ack:
                self.lose_clock_ack = False
                raise httpx.ReadTimeout("clock advanced before the response was lost")
        if path.startswith("/internal/v1/clock"):
            return httpx.Response(200, json={"mode": "BACKFILL", "business_time": self.at.isoformat()})
        if path == "/oauth/2.0/token":
            assert b"scope=card" in request.content
            return httpx.Response(200, json={"access_token": "network-token"})

        rid = request.headers["x-request-id"]
        op = next(
            op for op in self.truth.runs.rows["proof"]["operations"].values() if op["request_id"] == rid
        )
        assert op["state"] == "unconfirmed"
        (label,) = [row for row in self.truth.labels.rows.values() if row["request_id"] == rid]
        assert label == op["labels"][0]
        assert op["business_time"] == self.at.isoformat()
        self.calls.append(path)
        common = {"rsp_code": "A0000"}
        if path.endswith("/login"):
            assert label["subject_type"] == "customer"
            return httpx.Response(200, json=common | {"access_token": "holder-token"})
        assert label["subject_type"] == "account"
        if path != CARD_PATH:
            return httpx.Response(200, json=common | {"atm_session_token": "atm-token"})

        if self.lose_card_ack:
            raise httpx.ReadTimeout("card request could have posted")
        assert request.headers["authorization"] == "Bearer network-token"
        assert request.headers["x-sim-device"] == "card-network"
        payload = json.loads(request.content)
        (known_card,) = [row for row in self.truth.people if row["account_no"] == label["subject_id"]]
        assert payload.pop("card_no") == known_card["card_no"]
        assert payload == op["payload"]
        krw = card_amount_krw(payload["country"], payload["currency"], int(payload["amount"]))
        decline = payload["amount"] == "100" and label["label"] == "G"
        return httpx.Response(
            422 if decline else 200,
            json={
                "approved": not decline,
                "decline_reason": "insufficient_balance" if decline else None,
                "krw_amount": str(krw),
                "rsp_code": "A0004" if decline else "A0000",
                "bank_rsp_code": "405" if decline else "000",
            },
        )


def test_domestic_sampler_matches_configured_targets():
    assert sample_domestic_amount(Random(1)) == sample_domestic_amount(Random(1))
    rng = Random(1)
    amounts = sorted(sample_domestic_amount(rng) for _ in range(20_000))
    assert all(
        abs(amounts[len(amounts) * p // 100] - anchor) <= tolerance
        for (p, anchor), tolerance in zip(CARD_KRW_ANCHORS[1:-1], CARD_KRW_TOLERANCES, strict=True)
    )
    assert min(amounts) > 0


@pytest.mark.parametrize(
    ("country", "currency", "amount", "krw"),
    [
        ("KR", "KRW", 5, 5),
        ("JP", "JPY", 100, 900),
        ("JP", "USD", 2, 2600),
        ("CN", "CNY", 7, 1400),
        ("CN", "USD", 2, 2600),
        ("FR", "EUR", 3, 4200),
        ("FR", "USD", 2, 2600),
        ("US", "USD", 2, 2600),
    ],
)
def test_country_currency_matrix(country, currency, amount, krw):
    assert card_amount_krw(country, currency, amount) == krw


@pytest.mark.parametrize(
    ("country", "currency", "amount"),
    [
        ("JP", "JPY", 101),
        ("JP", "KRW", 100),
        ("US", "JPY", 100),
        ("KR", "USD", 1),
        ("KR", "KRW", 0),
        ("U1", "USD", 1),
        ("kr", "USD", 1),
        ("US", "USD", 999_999_999_999),
    ],
)
def test_invalid_card_amount_or_currency(country, currency, amount):
    with pytest.raises(ValueError):
        card_amount_krw(country, currency, amount)


def test_annual_cap_counts_planned_actions_in_separate_date_buckets():
    sample = fixture_plan(DAY, SEED)[-1]
    hundred = [replace(sample, at=sample.at + timedelta(days=i)) for i in range(100)]
    customer = {"normal": "reserved-customer"}
    december = [
        replace(action, at=action.at.replace(year=2027) if action.at.year == 2026 else action.at)
        for action in hundred
    ]
    enforce_normal_card_cap(december, customer)
    with pytest.raises(ValueError, match="annual cap"):
        enforce_normal_card_cap(december + [december[-1]], customer)
    enforce_normal_card_cap(december + [replace(december[-1], at=sample.at)], customer)


async def test_expected_decline_labels_and_run_summary_survive_repeat_read():
    fixture = Fixture()
    result = await fixture.run()
    assert result == await fixture.run()
    assert result["attempts"] == {"attempted": 10, "completed": 9, "declined": 1, "unresolved": 0}
    assert result["instances"] == {"G": 1, "lookalike": 2, "normal": 1}
    assert len(fixture.calls) == len(fixture.truth.labels.rows) == 10
    saved = repr(fixture.truth.runs.rows)
    for row in fixture.truth.people:
        assert row["password"] not in saved and row["card_no"] not in saved
    assert "network-token" not in saved and "atm-token" not in saved
    assert "fixture-network-secret" not in saved
    assert result["seed"] == SEED


async def test_clock_ack_loss_resumes_without_second_advance_at_same_instant():
    fixture = Fixture()
    fixture.lose_clock_ack = True
    with pytest.raises(httpx.ReadTimeout):
        await fixture.run()
    assert not fixture.calls
    (first_move,) = fixture.moves
    assert fixture.truth.runs.rows["proof"]["progress"]["intended"] == first_move
    await fixture.run()
    assert fixture.moves.count(first_move) == 1


async def test_clock_overshoot_halts_recovery_before_scenario_request():
    fixture = Fixture()
    fixture.lose_clock_ack = True
    with pytest.raises(httpx.ReadTimeout):
        await fixture.run()
    fixture.at += timedelta(seconds=1)
    with pytest.raises(RecoveryError, match="overshot"):
        await fixture.run()
    assert not fixture.calls


async def test_ambiguous_card_stops_all_resume_dispatch():
    fixture = Fixture()
    fixture.lose_card_ack = True
    with pytest.raises(RecoveryError, match="g_small is unconfirmed"):
        await fixture.run()
    assert fixture.calls.count(CARD_PATH) == 1
    with pytest.raises(RecoveryError, match="requires diagnosis"):
        await fixture.run()
    assert fixture.calls.count(CARD_PATH) == 1


async def test_backpressure_hysteresis_is_durable_across_restart(monkeypatch):
    fixture = Fixture()
    naps = []

    async def interrupted(_):
        naps.append(fixture.backlog)
        raise RecoveryError("interrupted during pause")

    monkeypatch.setattr(backfill_clock.asyncio, "sleep", interrupted)
    fixture.backlog = 50_001
    with pytest.raises(RecoveryError, match="during pause"):
        await fixture.run()
    assert fixture.truth.runs.rows["proof"]["progress"]["paused"]
    for fixture.backlog in (50_000, 30_000, 10_000):
        with pytest.raises(RecoveryError, match="during pause"):
            await fixture.run()
    assert not fixture.calls and not fixture.moves and len(naps) == 4
    fixture.backlog = 9_999
    await fixture.run()
    assert fixture.truth.runs.rows["proof"]["progress"]["paused"] is False


async def test_exact_pause_boundary_allows_new_run_at_50000():
    fixture = Fixture()
    fixture.backlog = 50_000
    assert (await fixture.run())["attempts"]["attempted"] == 10


async def test_resume_rejects_a_changed_plan_or_card_mapping(monkeypatch):
    fixture = Fixture()
    fixture.lose_clock_ack = True
    with pytest.raises(httpx.ReadTimeout):
        await fixture.run()

    plan = fixture_plan(DAY, SEED)
    with monkeypatch.context() as patch:
        patch.setattr(cards, "fixture_plan", lambda *_: plan + [plan[-1]] * 100)
        with pytest.raises(RecoveryError, match="run inputs differ"):
            await fixture.run()

    fixture.truth.people[0]["card_no"] = "0000999999999999"
    with pytest.raises(RecoveryError, match="card mapping changed"):
        await fixture.run()
    assert not fixture.calls


async def test_interrupted_backlog_observation_requires_draining_before_resume(monkeypatch):
    fixture = Fixture()
    fixture.backlog = 50_001
    fixture.lose_backlog_ack = True
    with pytest.raises(httpx.ReadTimeout):
        await fixture.run()
    assert fixture.truth.runs.rows["proof"]["progress"]["observing_backlog"]
    fixture.backlog = 30_000

    async def interrupted(_):
        raise RecoveryError("still waiting for drain")

    monkeypatch.setattr(backfill_clock.asyncio, "sleep", interrupted)
    with pytest.raises(RecoveryError, match="still waiting"):
        await fixture.run()
    assert not fixture.calls and not fixture.moves
    fixture.backlog = 9_999
    await fixture.run()


async def test_atm_session_refresh_after_interruption_is_separately_labelled(monkeypatch):
    fixture = Fixture()
    dispatch = cards._CardRun.dispatch

    async def interrupted(proof, action):
        if action.kind == "atm_withdraw":
            raise RecoveryError("interrupted before withdrawal")
        return await dispatch(proof, action)

    with monkeypatch.context() as patch:
        patch.setattr(cards._CardRun, "dispatch", interrupted)
        with pytest.raises(RecoveryError, match="interrupted before withdrawal"):
            await fixture.run()
    assert fixture.calls.count("/atm/v1/session") == 1

    summary = await fixture.run()
    assert fixture.calls.count("/atm/v1/session") == 2
    assert fixture.calls.count("/atm/v1/cash/withdraw") == 1
    assert summary["attempts"]["completed"] == 10
    first = summary["requests"]["g_atm_start"]
    second = summary["requests"]["g_atm_withdraw_refresh_1"]
    assert first != second
    matching = [row for row in fixture.truth.labels.rows.values() if row["request_id"] in {first, second}]
    assert len(matching) == 2
    assert {row["label"] for row in matching} == {"G"}
    assert {row["scenario_instance_id"] for row in matching} == {"proof:g"}
