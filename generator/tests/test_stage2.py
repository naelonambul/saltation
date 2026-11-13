import json
import re
from urllib.parse import parse_qs

import httpx
import pytest

from saltation_gen.cli import _main, build_parser
from saltation_gen.client.bank import BankClient, BankError, Context
from saltation_gen.drivers.seed import make_clients, make_counterparties, seed_parties

CTX = Context("127.0.0.1", "KR", "device")
RID = "01AAAAAAAAAAAAAAAAAAAAAAAA"
DT = "20260101000000"
AUTH = (RID, "token", CTX)
FIN = "1" * 24
CASES = [
    (
        "seed_counterparties",
        ([{"name": "김민준"}],),
        "POST",
        "/internal/v1/seed/counterparties",
        {"counterparties": [{"name": "김민준"}]},
        (200,),
        False,
    ),
    (
        "seed_counterparty_clients",
        ([],),
        "POST",
        "/internal/v1/seed/counterparty_clients",
        {"counterparty_clients": []},
        (200,),
        False,
    ),
    ("clock", (), "GET", "/internal/v1/clock", None, (200,), False),
    (
        "clock_set",
        ("2026-01-01T00:00:00+09:00",),
        "POST",
        "/internal/v1/clock/set",
        {"business_time": "2026-01-01T00:00:00+09:00"},
        (200,),
        False,
    ),
    ("clock_advance", (60,), "POST", "/internal/v1/clock/advance", {"seconds": 60}, (200,), False),
    ("go_live", (), "POST", "/internal/v1/mode/live", None, (200,), False),
    ("sms", ("01012345678",), "GET", "/internal/v1/sms/01012345678", None, (200, 404), False),
    (
        "client_token",
        ("cp-901", "secret"),
        "POST",
        "/oauth/2.0/token",
        {
            "client_id": "cp-901",
            "client_secret": "secret",
            "scope": "oob",
            "grant_type": "client_credentials",
        },
        (200,),
        False,
    ),
    (
        "inbound_deposit",
        (*AUTH, "901123", FIN, 100, "김민준", DT, "memo"),
        "POST",
        "/v2.0/transfer/deposit/fin_num",
        {
            "cntr_account_type": "N",
            "cntr_account_num": "901123",
            "wd_print_content": "memo",
            "name_check_option": "off",
            "tran_dtime": DT,
            "req_cnt": "1",
            "req_list": [
                {
                    "tran_no": "1",
                    "fintech_use_num": FIN,
                    "print_content": "memo",
                    "tran_amt": "100",
                    "req_client_name": "김민준",
                    "transfer_purpose": "TR",
                }
            ],
        },
        (200, 422),
        True,
    ),
    (
        "inquiry_receive",
        (*AUTH, "901", "901123", 100, DT, "memo"),
        "POST",
        "/v2.0/inquiry/receive",
        {
            "bank_code_std": "901",
            "account_num": "901123",
            "tran_amt": "100",
            "tran_dtime": DT,
            "print_content": "memo",
        },
        (200, 422),
        True,
    ),
    ("payees", ("token",), "GET", "/v2.0/ext/payees", None, (200,), True),
    (
        "register_payee",
        (*AUTH, "901", "901123", "alias"),
        "POST",
        "/v2.0/ext/payees",
        {"bank_code_std": "901", "account_num": "901123", "alias": "alias"},
        (200, 409, 422),
        True,
    ),
    ("delete_payee", (*AUTH, "12"), "DELETE", "/v2.0/ext/payees", None, (200, 404), True),
    ("products", ("token",), "GET", "/v2.0/ext/products", None, (200,), True),
    (
        "open_deposit",
        (*AUTH, FIN, "2", 100, DT),
        "POST",
        "/v2.0/ext/deposits",
        {"fintech_use_num": FIN, "terms_id": "2", "tran_amt": "100", "tran_dtime": DT},
        (200, 404, 422),
        True,
    ),
    ("deposit", ("token", "12"), "GET", "/v2.0/ext/deposits/12", None, (200,), True),
    (
        "close_deposit",
        (*AUTH, "12", DT),
        "POST",
        "/v2.0/ext/deposits/12/close",
        {"tran_dtime": DT},
        (200, 404, 422),
        True,
    ),
    (
        "take_loan",
        (*AUTH, FIN, "2", 100, DT),
        "POST",
        "/v2.0/ext/loans",
        {"fintech_use_num": FIN, "terms_id": "2", "tran_amt": "100", "tran_dtime": DT},
        (200, 404, 422),
        True,
    ),
    ("loan", ("token", "12"), "GET", "/v2.0/ext/loans/12", None, (200,), True),
    (
        "repay_loan",
        (*AUTH, "12", 100, DT),
        "POST",
        "/v2.0/ext/loans/12/repay",
        {"tran_amt": "100", "tran_dtime": DT},
        (200, 404, 422),
        True,
    ),
    (
        "password_reset_request",
        (RID, "01012345678", CTX),
        "POST",
        "/v2.0/ext/auth/password_reset/request",
        {"phone": "01012345678"},
        (200,),
        False,
    ),
    (
        "password_reset_confirm",
        (RID, "01012345678", "123456", "password", CTX),
        "POST",
        "/v2.0/ext/auth/password_reset/confirm",
        {"phone": "01012345678", "code": "123456", "new_password": "password"},
        (200, 401),
        False,
    ),
]


@pytest.mark.parametrize("name,args,method,path,payload,statuses,bearer", CASES)
async def test_client_contract(name, args, method, path, payload, statuses, bearer):
    requests = []
    status = 200
    body = {"created": 3, "access_token": "access", "res_list": [{"id": "12"}]}

    def handler(request):
        requests.append(request)
        return httpx.Response(status, json=body)

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    try:
        for status in statuses:
            result = await getattr(bank, name)(*args)
            req = requests[-1]
            assert req.method == method and req.url.path == path
            assert re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", req.headers["X-Request-Id"])
            if RID in args:
                assert req.headers["X-Request-Id"] == RID
                assert req.headers["X-Sim-Device"] == CTX.device
            assert req.headers.get("Authorization") == ("Bearer token" if bearer else None)
            if name == "client_token":
                assert req.headers["content-type"].startswith("application/x-www-form-urlencoded")
                assert parse_qs(req.content.decode()) == {k: [v] for k, v in payload.items()}
            elif payload is not None:
                actual = json.loads(req.content)
                transaction = actual["req_list"][0] if name == "inbound_deposit" else actual
                if "bank_tran_id" in transaction:
                    assert re.fullmatch(r"[A-Za-z0-9]{10}[UM][A-Za-z0-9]{9}", transaction.pop("bank_tran_id"))
                assert actual == payload
            else:
                assert not req.content
            assert dict(req.url.params) == ({"payee_id": "12"} if name == "delete_payee" else {})
            if name.startswith("seed_"):
                assert result == 3
            elif name == "client_token":
                assert result == "access"
            elif name in ("payees", "products"):
                assert result == body["res_list"]
            elif name == "sms" and status == 404:
                assert result is None
            elif name in ("clock", "clock_set", "clock_advance", "go_live", "sms", "deposit", "loan"):
                assert result == body
            else:
                assert result == (status, body)
        status = 500
        with pytest.raises(BankError) as error:
            await getattr(bank, name)(*args)
        assert error.value.status == 500
    finally:
        await bank.close()


async def test_transfer_business_time_and_bank():
    requests = []

    def handler(req):
        requests.append(req)
        return httpx.Response(200, json={})

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    try:
        await bank.transfer(*AUTH, FIN, "901123", "김민준", 100, tran_dtime=DT, bank_code="901")
        body = json.loads(requests[0].content)
        assert body["tran_dtime"] == DT
        assert body["recv_client_bank_code"] == "901"
        assert body["transfer_purpose"] == "TR"
    finally:
        await bank.close()


def test_party_generation():
    parties = make_counterparties(7, 500)
    clients = make_clients(7)
    assert parties == make_counterparties(7, 500)
    assert clients == make_clients(7)
    assert clients != make_clients(8)
    assert len({p["account_no"] for p in parties}) == 500
    assert len({p["client_id"] for p in clients}) == 3
    for i, party in enumerate(parties):
        assert re.fullmatch(r"^[0-9]{1,16}$", party["account_no"])
        assert party["institution_code"] == str(901 + i % 3)
        assert re.fullmatch(r"^[0-9]{3}$", party["institution_code"])
        assert party["counterparty_type"] == ("EMPLOYER", "MERCHANT", "INDIVIDUAL")[i % 3]
        assert 0 < len(party["name"]) <= 50
        assert re.fullmatch(r"[가-힣]+", party["name"])
        if "industry_code" in party:
            assert re.fullmatch(r"^[A-Za-z0-9]{1,10}$", party["industry_code"])
    for client in clients:
        assert re.fullmatch(r"^[A-Za-z0-9_-]{1,40}$", client["client_id"])
        assert re.fullmatch(r"^[0-9]{3}$", client["institution_code"])
        assert 32 <= len(client["client_secret"]) <= 128
        assert client["scope"] == "oob"
    assert {c["institution_code"] for c in clients} == {"901", "902", "903"}


async def test_seed_party_batches():
    batches = []

    def handler(req):
        values = next(iter(json.loads(req.content).values()))
        batches.append((req.url.path, len(values)))
        return httpx.Response(200, json={"created": len(values)})

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    try:
        assert await seed_parties(bank, 7, 405) == {"counterparties": 405, "counterparty_clients": 3}
        assert [n for _, n in batches] == [200, 200, 5, 3]
    finally:
        await bank.close()


@pytest.mark.parametrize(
    "argv",
    [
        ["clock", "show"],
        ["clock", "set", "2026-01-01T00:00:00+09:00"],
        ["clock", "advance", "60"],
        ["clock", "live"],
        ["seed-parties", "--counterparties", "5"],
    ],
)
async def test_new_commands_without_truth(argv, monkeypatch, capsys):
    from saltation_gen import cli

    args = build_parser().parse_args(argv)
    calls = []

    class FakeBank:
        def __init__(self, url):
            pass

        async def close(self):
            pass

        async def clock(self):
            calls.append(("show",))
            return {"mode": "BACKFILL", "business_time": DT, "rolled_through": None}

        async def clock_set(self, value):
            calls.append(("set", value))
            return await self.clock()

        async def clock_advance(self, value):
            calls.append(("advance", value))
            return await self.clock()

        async def go_live(self):
            calls.append(("live",))
            return await self.clock()

        async def seed_counterparties(self, items):
            return len(items)

        async def seed_counterparty_clients(self, items):
            return len(items)

    def forbidden(*args):
        raise AssertionError("truth store opened")

    monkeypatch.setattr(cli, "BankClient", FakeBank)
    monkeypatch.setattr(cli, "TruthStore", forbidden)
    await _main(args)
    result = json.loads(capsys.readouterr().out)
    if argv[0] == "clock":
        assert calls[0] == ((argv[1], int(argv[2])) if argv[1] == "advance" else tuple(argv[1:]))
        assert set(result) == {"mode", "business_time", "rolled_through"}
    else:
        assert result == {"counterparties": 5, "counterparty_clients": 3}
