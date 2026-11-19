import json
import re
from urllib.parse import parse_qs

import httpx
import pytest

from saltation_gen.client import BankClient, BankError, new_ulid
from saltation_gen.client.bank import Context
from saltation_gen.config import ATM_TERMINALS
from saltation_gen.drivers.seed import (
    CARD_FIXTURE_START,
    CARD_NETWORK_ID,
    make_clients,
    make_customers,
    seed_card_fixture,
)

CTX = Context("192.0.2.1", "KR-11", "synthetic-card-device")
CARDS = ["0000000000000018", "0000000000000026", "0000000000000034", "0000000000000042"]
TOKEN = "a" * 43


@pytest.fixture
async def bank_for():
    clients = []

    def make(handler):
        client = BankClient("http://generator.test:8081", transport=httpx.MockTransport(handler))
        clients.append(client)
        return client

    yield make
    for client in clients:
        await client.close()


def assert_context(request, request_id):
    assert request.headers["X-Request-Id"] == request_id
    assert request.headers["X-Sim-Ip"] == CTX.ip
    assert request.headers["X-Sim-Region"] == CTX.region
    assert request.headers["X-Sim-Device"] == CTX.device


@pytest.mark.parametrize("status", [200, 422, 201, 400, 401, 403, 409, 503])
async def test_card_authorization_wire(bank_for, status):
    request_id = new_ulid()
    payload = {
        "idempotency_key": "synthetic-payment-1",
        "card_no": CARDS[0],
        "amount": "1200",
        "currency": "KRW",
        "merchant_name": " 테스트 가게 ",
        "mcc": "0123",
        "country": "KR",
        "entry_mode": "in_person",
        "latitude": "37.1200",
        "longitude": "127.34",
    }
    response = {"request_id": request_id, "rsp_code": {200: "A0000", 422: "A0004"}.get(status, "A0002")}

    def handler(request):
        assert (request.method, request.url.path) == ("POST", "/card/v1/authorize")
        assert_context(request, request_id)
        bearer_matches = request.headers.get("Authorization") == f"Bearer {TOKEN}"
        assert bearer_matches
        assert request.headers["Content-Type"] == "application/json"
        assert json.loads(request.content) == payload
        return httpx.Response(status, json=response)

    bank = bank_for(handler)
    if status in (200, 422):
        assert await bank.card_authorize(request_id, TOKEN, CTX, payload) == (status, response)
    else:
        with pytest.raises(BankError) as exc:
            await bank.card_authorize(request_id, TOKEN, CTX, payload)
        assert (exc.value.status, exc.value.body) == (status, response)


@pytest.mark.parametrize("scope", ["oob", "card"])
async def test_client_token_wire(bank_for, scope):
    request_id = new_ulid()
    client_id, secret = (
        (CARD_NETWORK_ID, "synthetic-card-secret") if scope == "card" else ("cp-901", "synthetic-oob-secret")
    )

    def handler(request):
        assert (request.method, request.url.path) == ("POST", "/oauth/2.0/token")
        assert request.headers["Content-Type"] == "application/x-www-form-urlencoded"
        assert "Authorization" not in request.headers
        if scope == "card":
            assert_context(request, request_id)
        else:
            assert re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", request.headers["X-Request-Id"])
            assert all(not name.startswith("x-sim-") for name in request.headers)
        form = parse_qs(request.content.decode())
        secret_matches = form.pop("client_secret", None) == [secret]
        assert secret_matches
        assert form == {"client_id": [client_id], "scope": [scope], "grant_type": ["client_credentials"]}
        return httpx.Response(200, json={"access_token": TOKEN})

    bank = bank_for(handler)
    kwargs = {"scope": scope, "request_id": request_id, "ctx": CTX} if scope == "card" else {}
    token_matches = await bank.client_token(client_id, secret, **kwargs) == TOKEN
    assert token_matches


@pytest.mark.parametrize(
    "failure",
    [
        None,
        "unknown_account",
        "duplicate_account",
        "unknown_card_account",
        "duplicate_card_account",
        "missing_card",
        "nonsynthetic_card",
        "invalid_luhn",
        "duplicate_card_no",
        "blocked_card",
        "client_seed_error",
    ],
)
async def test_card_fixture_wire_and_roster(bank_for, failure):
    seed_value = 123
    expected = make_customers(seed_value, 4, CARD_FIXTURE_START)
    balances = [100_000, 1_000_000, 1_000_000, 1_000_000]
    customer_nos = [f"C{i}" for i in range(97_100_000, 97_100_004)]
    account_nos = [f"998{i}" for i in range(97_100_000, 97_100_004)]
    fins = [f"{i:024}" for i in range(1, 5)]
    calls = []
    entries = []
    seed_paths = ["customers", "accounts", "cards", "atm_terminals", "counterparty_clients"]

    def handler(request):
        name = request.url.path.removeprefix("/internal/v1/seed/")
        assert name == seed_paths[len(calls)]
        calls.append(name)
        assert request.method == "POST"
        assert re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", request.headers["X-Request-Id"])
        assert "Authorization" not in request.headers
        assert request.headers["Content-Type"] == "application/json"
        items = json.loads(request.content)[name]
        if name == "customers":
            assert [item["customer_no"] for item in items] == customer_nos
            return httpx.Response(200, json={"created": 4})
        if name == "accounts":
            assert [item["account_no"] for item in items] == account_nos
            assert [item["customer_no"] for item in items] == customer_nos
            assert [item["opening_balance"] for item in items] == balances
            pins_match = [item["atm_pin"] for item in items] == [c["atm_pin"] for c in expected]
            assert pins_match
            rows = [
                {"account_no": account, "fintech_use_num": fin}
                for account, fin in zip(account_nos, fins, strict=True)
            ]
            if failure == "unknown_account":
                rows[0]["account_no"] = account_nos[0] + "0"
            elif failure == "duplicate_account":
                rows[0]["account_no"] = account_nos[1]
            return httpx.Response(200, json={"accounts": rows[::-1]})
        if name == "cards":
            assert items == [{"account_no": account, "status": "ACTIVE"} for account in account_nos]
            rows = [{**item, "card_no": number} for item, number in zip(items, CARDS, strict=True)]
            if failure == "unknown_card_account":
                rows[0]["account_no"] = account_nos[0] + "0"
            elif failure == "duplicate_card_account":
                rows[0]["account_no"] = account_nos[1]
            elif failure == "missing_card":
                rows.pop()
            elif failure == "nonsynthetic_card":
                rows[0]["card_no"] = "9" * 16
            elif failure == "invalid_luhn":
                rows[0]["card_no"] = "0000000000000010"
            elif failure == "duplicate_card_no":
                rows[0]["card_no"] = CARDS[1]
            elif failure == "blocked_card":
                rows[0]["status"] = "BLOCKED"
            return httpx.Response(200, json={"cards": rows[1:] + rows[:1]})
        if name == "atm_terminals":
            assert items == ATM_TERMINALS
            return httpx.Response(200, json={"atm_terminals": items})
        assert len(items) == 1
        seeded = items[0]
        client_id = seeded["client_id"]
        assert client_id == CARD_NETWORK_ID
        assert client_id not in {item["client_id"] for item in make_clients(seed_value)}
        assert seeded["scope"] == "card"
        assert re.fullmatch(r"[0-9]{3}", seeded["institution_code"])
        assert bool(re.fullmatch(r"[0-9a-f]{64}", seeded["client_secret"]))
        truth.seeded_secret = seeded["client_secret"]
        if failure == "client_seed_error":
            return httpx.Response(503, json={"rsp_code": "A0013"})
        return httpx.Response(200, json={"created": 1})

    class Truth:
        def __init__(self):
            self.roster = self
            self.card_networks = self
            self.saved_secret = None

        async def replace_one(self, query, doc, *, upsert):
            assert query == {"_id": CARD_NETWORK_ID}
            assert doc["_id"] == CARD_NETWORK_ID and upsert
            self.saved_secret = doc["client_secret"]

        async def delete_many(self, query):
            assert calls == seed_paths
            assert query == {"customer_no": {"$in": customer_nos}}
            calls.append("delete")

        async def add_roster(self, items):
            assert calls[-1] == "delete"
            entries.extend(items)

    bank, truth = bank_for(handler), Truth()
    if failure:
        error = BankError if failure == "client_seed_error" else ValueError
        with pytest.raises(error):
            await seed_card_fixture(bank, truth, seed_value)
        assert "delete" not in calls
        assert not entries
        assert truth.saved_secret is None
    else:
        await seed_card_fixture(bank, truth, seed_value)
        assert calls == [*seed_paths, "delete"]
        expected_entries = [
            {**c, "opening_balance": balance, "fintech_use_num": fin, "card_no": card}
            for c, balance, fin, card in zip(expected, balances, fins, CARDS, strict=True)
        ]
        roster_matches = entries == expected_entries
        assert roster_matches
        assert bool(truth.saved_secret == truth.seeded_secret)
        assert all("client_secret" not in entry for entry in entries)
