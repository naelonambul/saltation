import json
import logging
import re
from decimal import Decimal

import httpx
import pytest

from saltation_gen.client.bank import BankClient, BankError, Context, atm_idempotency_key
from saltation_gen.config import ATM_TERMINALS
from saltation_gen.drivers.seed import make_customers, seed, seed_terminals

CTX = Context("127.0.0.1", "KR-11", "atm-device")
RID = "01AAAAAAAAAAAAAAAAAAAAAAAA"
TOKEN = "opaque-atm-secret"
PIN = "0268"
KEY = "0123456789ABCDEFGHIJ"
CASES = [
    (
        "seed_atm_terminals",
        (ATM_TERMINALS,),
        "POST",
        "/internal/v1/seed/atm_terminals",
        {"atm_terminals": ATM_TERMINALS},
        (200,),
        False,
    ),
    (
        "atm_start_session",
        (RID, "ATM-11-001", "998123", PIN, CTX),
        "POST",
        "/atm/v1/session",
        {"terminal_id": "ATM-11-001", "account_no": "998123", "pin": PIN},
        (200, 400, 404, 422, 503),
        False,
    ),
    ("atm_end_session", (RID, TOKEN, CTX), "POST", "/atm/v1/session/end", None, (200, 400, 401, 503), True),
    (
        "atm_withdraw",
        (RID, TOKEN, KEY, 10000, CTX),
        "POST",
        "/atm/v1/cash/withdraw",
        {"idempotency_key": KEY, "amount": "10000"},
        (200, 400, 401, 409, 422, 503),
        True,
    ),
    (
        "atm_deposit",
        (RID, TOKEN, KEY, 10000, CTX),
        "POST",
        "/atm/v1/cash/deposit",
        {"idempotency_key": KEY, "amount": "10000"},
        (200, 400, 401, 409, 422, 503),
        True,
    ),
    ("atm_balance", (RID, TOKEN, CTX), "GET", "/atm/v1/balance", None, (200, 400, 401, 422, 503), True),
]


@pytest.mark.parametrize("name,args,method,path,payload,statuses,bearer", CASES)
async def test_atm_client_contract(name, args, method, path, payload, statuses, bearer, caplog, capsys):
    requests = []
    status = 200
    body = {"atm_terminals": [{"terminal_id": "ATM-11-001"}], "atm_session_token": TOKEN}

    def handler(request):
        requests.append(request)
        return httpx.Response(status, json=body)

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    caplog.set_level(logging.DEBUG, logger="saltation_gen.client")
    try:
        for status in statuses:
            result = await getattr(bank, name)(*args)
            request = requests[-1]
            assert request.method == method
            assert request.url.path == path
            assert not request.url.params
            assert request.headers.get("Authorization") == (f"Bearer {TOKEN}" if bearer else None)
            if name == "seed_atm_terminals":
                assert re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", request.headers["X-Request-Id"])
                assert all(f"X-Sim-{field}" not in request.headers for field in ("Ip", "Region", "Device"))
                assert result == 1
            else:
                assert request.headers["X-Request-Id"] == RID
                assert request.headers["X-Sim-Ip"] == CTX.ip
                assert request.headers["X-Sim-Region"] == CTX.region
                assert request.headers["X-Sim-Device"] == CTX.device
                assert result == (status, body)
            if payload is None:
                assert not request.content
            else:
                assert json.loads(request.content) == payload
        status = 500
        with pytest.raises(BankError) as error:
            await getattr(bank, name)(*args)
        assert error.value.status == 500
        assert not caplog.records
        captured = capsys.readouterr()
        assert not captured.out and not captured.err
    finally:
        await bank.close()


def test_atm_idempotency_keys():
    keys = [atm_idempotency_key() for _ in range(1000)]
    assert len(set(keys)) == 1000
    assert all(re.fullmatch(r"[ -~]{20}", key) for key in keys)


def test_terminal_contract():
    regions = {
        "KR-11",
        "KR-26",
        "KR-27",
        "KR-28",
        "KR-29",
        "KR-30",
        "KR-31",
        "KR-41",
        "KR-42",
        "KR-43",
        "KR-44",
        "KR-45",
        "KR-46",
        "KR-47",
        "KR-48",
        "KR-49",
        "KR-50",
    }
    assert len(ATM_TERMINALS) == 17
    assert {t["region"] for t in ATM_TERMINALS} == regions
    assert len({t["terminal_id"] for t in ATM_TERMINALS}) == 17
    # Patterns from internal.yaml SeedAtmTerminalsRequest (no YAML/schema test dependency).
    patterns = {
        "terminal_id": r"^[\x21-\x7E]{1,20}$",
        "region": r"^KR-(1[1-9]|[2-4][0-9]|50)$",
        "latitude": r"^(3[3-7](\.[0-9]{1,6})?|38(\.([0-6][0-9]{0,5}|70{0,5}))?)$",
        "longitude": r"^(124\.[5-9][0-9]{0,5}|12[5-9](\.[0-9]{1,6})?|13[01](\.[0-9]{1,6})?|132(\.0{1,6})?)$",
    }
    for terminal in ATM_TERMINALS:
        assert set(terminal) == {*patterns, "location"}
        assert all(isinstance(value, str) for value in terminal.values())
        for field, pattern in patterns.items():
            assert re.fullmatch(pattern, terminal[field])
        assert terminal["terminal_id"] == f"ATM-{terminal['region'][3:]}-001"
        assert 1 <= len(terminal["location"]) <= 60
        assert re.fullmatch(r"[가-힣]+", terminal["location"])
        for field, lower, upper in (("latitude", "33", "38.7"), ("longitude", "124.5", "132")):
            assert re.fullmatch(r"[0-9]+\.[0-9]{6}", terminal[field])
            assert Decimal(lower) <= Decimal(terminal[field]) <= Decimal(upper)


async def test_seed_terminals_single_request():
    requests = []

    def handler(request):
        requests.append(request)
        assert json.loads(request.content) == {"atm_terminals": ATM_TERMINALS}
        return httpx.Response(200, json={"atm_terminals": [{"terminal_id": "ATM-11-001"}]})

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    try:
        assert await seed_terminals(bank) == 1
        assert len(requests) == 1
        assert requests[0].url.path == "/internal/v1/seed/atm_terminals"
    finally:
        await bank.close()


async def test_seed_retains_pin_in_account_payload_and_roster():
    accounts = []
    roster = []

    class FakeBank:
        async def seed_customers(self, items):
            assert all("atm_pin" not in item for item in items)

        async def seed_accounts(self, items):
            accounts.extend(items)
            return [
                {"account_no": item["account_no"], "fintech_use_num": str(i)} for i, item in enumerate(items)
            ]

    class FakeTruth:
        async def add_roster(self, items):
            roster.extend(items)

    assert await seed(FakeBank(), FakeTruth(), 7, 3) == 3
    expected = make_customers(7, 3)
    assert len(accounts) == len(roster) == 3
    for customer, account, entry in zip(expected, accounts, roster, strict=True):
        assert account["atm_pin"] == entry["atm_pin"] == customer["atm_pin"]
        assert account["account_no"] == entry["account_no"] == customer["account_no"]
        assert re.fullmatch(r"[0-9]{4}", entry["atm_pin"])
