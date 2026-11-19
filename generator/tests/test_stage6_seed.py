import json
import re
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock, call

import httpx
import pytest

from saltation_gen.client.bank import BankClient, Context
from saltation_gen.config import ATM_TERMINALS
from saltation_gen.drivers import stage6_seed
from saltation_gen.drivers.seed import BATCH
from saltation_gen.drivers.stage6_seed import CARD_CLIENT_ID, OOB_CLIENT_ID, SENDER_ACCOUNTS

PREPARATION = datetime.fromisoformat("2024-01-01T00:00:00+09:00")
FIRST = datetime.fromisoformat("2025-01-02T00:00:00+09:00")


@pytest.mark.parametrize("explicit", [True, False])
async def test_transaction_keys_on_wire(monkeypatch, explicit):
    generated = "M2026SALTGU000000001"
    persisted = "M2026SALTGU000000002"
    default_key = Mock(return_value=generated)
    monkeypatch.setattr("saltation_gen.client.bank.bank_tran_id", default_key)
    requests = []
    balance_status = 200 if explicit else 404

    def handler(request):
        requests.append(request)
        return httpx.Response(balance_status if request.method == "GET" else 200, json={"rsp_code": "test"})

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    ctx = Context("192.0.2.10", "KR-11", "device")
    common = ("test-request", "token", ctx)
    when = "20250102000000"
    fintech = "998000000000000000000001"
    kwargs = {"transaction_key": persisted} if explicit else {}
    try:
        assert await bank.balance(*common, fintech, when, **kwargs) == (balance_status, {"rsp_code": "test"})
        await bank.inbound_deposit(
            *common, SENDER_ACCOUNTS["salary"], fintech, 5000, "sender", when, **kwargs
        )
        await bank.open_deposit(*common, fintech, "1", 5000, when, **kwargs)
        await bank.close_deposit(*common, "123", when, **kwargs)
    finally:
        await bank.close()

    assert [(req.method, req.url.path) for req in requests] == [
        ("GET", "/v2.0/account/balance/fin_num"),
        ("POST", "/v2.0/transfer/deposit/fin_num"),
        ("POST", "/v2.0/ext/deposits"),
        ("POST", "/v2.0/ext/deposits/123/close"),
    ]
    key = persisted if explicit else generated
    balance, inbound, opening, closing = requests
    assert dict(balance.url.params) == {"bank_tran_id": key, "fintech_use_num": fintech, "tran_dtime": when}
    assert not balance.content
    assert json.loads(inbound.content)["req_list"][0]["bank_tran_id"] == key
    assert json.loads(opening.content)["bank_tran_id"] == key
    assert json.loads(closing.content)["bank_tran_id"] == key
    assert default_key.call_count == (0 if explicit else 4)
    for req in requests:
        assert req.headers["X-Request-Id"] == "test-request"
        assert req.headers["Authorization"] == "Bearer token"
        assert req.headers["X-Sim-Ip"] == ctx.ip
        assert req.headers["X-Sim-Region"] == ctx.region
        assert req.headers["X-Sim-Device"] == ctx.device


async def test_sms_and_analyst_wire_contract():
    requests = []
    analyst = {"analyst_no": "test.1", "name": "Analyst", "password": "synthetic"}

    def handler(request):
        requests.append(request)
        if request.url.path == "/internal/v1/seed/analysts":
            return httpx.Response(200, json={"created": 1})
        if request.headers.get("X-Request-Id") == "sms-request":
            return httpx.Response(200, json={"code": "123456"})
        return httpx.Response(404)

    bank = BankClient("http://bank", transport=httpx.MockTransport(handler))
    ctx = Context("192.0.2.11", "KR-26", "second-device")
    try:
        assert await bank.sms("01000000001", request_id="sms-request", ctx=ctx) == {"code": "123456"}
        assert await bank.sms("01000000002") is None
        assert await bank.seed_analysts([analyst]) == 1
    finally:
        await bank.close()
    labeled, default, seed = requests
    assert labeled.url.path == "/internal/v1/sms/01000000001"
    assert labeled.headers["X-Sim-Ip"] == ctx.ip
    assert labeled.headers["X-Sim-Region"] == ctx.region
    assert labeled.headers["X-Sim-Device"] == ctx.device
    assert default.url.path == "/internal/v1/sms/01000000002"
    assert re.fullmatch("[0-9A-HJKMNP-TV-Z]{26}", default.headers["X-Request-Id"])
    assert "X-Sim-Ip" not in default.headers
    assert seed.method == "POST"
    assert seed.url.path == "/internal/v1/seed/analysts"
    assert json.loads(seed.content) == {"analysts": [analyst]}


def _card_number(account):
    prefix = f"0000{int(account[-8:]):011d}"
    total = sum(
        (value * 2 - 9 * (value >= 5)) if i % 2 == 0 else value for i, value in enumerate(map(int, prefix))
    )
    return prefix + str(-total % 10)


def _accounts(rows):
    return [
        {"account_no": row["account_no"], "fintech_use_num": f"{int(row['account_no']):024d}"}
        for row in reversed(rows)
    ]


def _cards(rows):
    return [{**row, "card_no": _card_number(row["account_no"])} for row in reversed(rows)]


def _bank():
    bank = AsyncMock(spec=BankClient)
    bank.seed_accounts.side_effect = _accounts
    bank.seed_cards.side_effect = _cards
    bank.seed_counterparties.side_effect = len
    bank.seed_counterparty_clients.side_effect = len
    bank.seed_analysts.side_effect = len
    return bank


def _truth(*, roster=(), networks=()):
    roster_rows = list(roster)
    network_rows = list(networks)

    async def roster_find(query, projection):
        return next(
            (
                row
                for row in roster_rows
                if any(
                    row.get(field) in match["$in"]
                    for clause in query["$or"]
                    for field, match in clause.items()
                )
            ),
            None,
        )

    async def network_find(query, projection):
        return next((row for row in network_rows if row["_id"] in query["_id"]["$in"]), None)

    async def network_insert(rows, *, ordered):
        assert ordered
        network_rows.extend(rows)

    async def roster_insert(rows):
        roster_rows.extend(rows)

    return SimpleNamespace(
        roster=SimpleNamespace(find_one=roster_find),
        card_networks=SimpleNamespace(find_one=network_find, insert_many=network_insert),
        add_roster=roster_insert,
        roster_rows=roster_rows,
        network_rows=network_rows,
    )


def test_world_customer_number_bounds():
    assert stage6_seed.world_customer_numbers(3, 7) == ["C00000007", "C00000008", "C00000009"]
    assert stage6_seed.world_customer_numbers(1, 99_999_999) == ["C99999999"]
    assert stage6_seed.world_customer_numbers(0) == []
    for population, start in [(-1, 0), (1, -1), (2, 99_999_999)]:
        with pytest.raises(ValueError, match="eight digits"):
            stage6_seed.world_customer_numbers(population, start)


async def test_seed_world_orders_cohorts_bounds_batches_and_preserves_other_rosters(monkeypatch):
    start, population = 20, 2 * BATCH + 5
    numbers = stage6_seed.world_customer_numbers(population, start)
    dormant = set(numbers[: BATCH + 1]) | {numbers[-1]}
    other = {"customer_no": "C98765432", "account_no": "99898765432", "protected": True}
    other_network = {"_id": "card-network", "client_secret": "fixture-only"}
    truth = _truth(roster=[other], networks=[other_network])
    bank = _bank()
    make = Mock(wraps=stage6_seed.make_customers)
    monkeypatch.setattr(stage6_seed, "make_customers", make)
    analysts = [
        {"analyst_no": f"test.{i}", "name": "Analyst", "password": "synthetic", "extra": "local"}
        for i in range(101)
    ]

    await stage6_seed.seed_world(bank, truth, 7, population, start, dormant, PREPARATION, FIRST, analysts)

    assert bank.method_calls[0] == call.clock_set(PREPARATION.isoformat())
    assert bank.method_calls[10] == call.clock_set(FIRST.isoformat())
    assert [entry[0] for entry in bank.method_calls[1:10]] == [
        "seed_customers",
        "seed_accounts",
        "seed_cards",
    ] * 3
    assert [entry[0] for entry in bank.method_calls[11:17]] == [
        "seed_customers",
        "seed_accounts",
        "seed_cards",
    ] * 2
    customer_batches = [item.args[0] for item in bank.seed_customers.await_args_list]
    account_batches = [item.args[0] for item in bank.seed_accounts.await_args_list]
    card_batches = [item.args[0] for item in bank.seed_cards.await_args_list]
    assert [len(batch) for batch in account_batches] == [BATCH, 1, 1, BATCH - 1, 4]
    for customers, accounts, cards in zip(customer_batches, account_batches, card_batches, strict=True):
        assert [row["customer_no"] for row in customers] == [row["customer_no"] for row in accounts]
        assert cards == [{"account_no": row["account_no"], "status": "ACTIVE"} for row in accounts]
        for row in accounts:
            assert row["opening_balance"] == (0 if row["customer_no"] in dormant else 3_000_000)
    assert all(0 < item.args[1] <= BATCH for item in make.call_args_list)
    make.assert_any_call(7 + start, BATCH, start)

    assert truth.roster_rows[0] == other
    rows = truth.roster_rows[1:]
    assert len(rows) == population and {row["customer_no"] for row in rows} == set(numbers)
    customers = {row["customer_no"]: row for batch in customer_batches for row in batch}
    accounts = {row["customer_no"]: row for batch in account_batches for row in batch}
    for row in rows:
        customer, account = customers[row["customer_no"]], accounts[row["customer_no"]]
        assert row["protected"] is True
        assert all(row[field] == customer[field] for field in ("name", "phone", "password"))
        assert all(row[field] == account[field] for field in ("atm_pin", "account_no"))
        assert row["fintech_use_num"] == _accounts([account])[0]["fintech_use_num"]
        assert row["card_no"] == _card_number(row["account_no"])
        assert "client_secret" not in row

    bank.seed_atm_terminals.assert_awaited_once_with(ATM_TERMINALS)
    counterparties = bank.seed_counterparties.await_args.args[0]
    assert {row["institution_code"] for row in counterparties} == {"901"}
    assert {row["counterparty_type"]: row["account_no"] for row in counterparties} == {
        "EMPLOYER": SENDER_ACCOUNTS["salary"],
        "MERCHANT": SENDER_ACCOUNTS["settlement"],
        "INDIVIDUAL": SENDER_ACCOUNTS["unrelated"],
    }
    assert all(re.fullmatch(r"[0-9]{16}", row["account_no"]) for row in counterparties)
    assert len({row["account_no"] for row in counterparties}) == 3
    assert truth.network_rows[0] == other_network
    card, oob = truth.network_rows[1:]
    assert card["_id"] == CARD_CLIENT_ID and oob["_id"] == OOB_CLIENT_ID
    assert oob["sender_accounts"] == SENDER_ACCOUNTS
    assert card["client_secret"] != oob["client_secret"]
    assert bank.seed_counterparty_clients.await_args.args[0] == [
        {
            "client_id": row["_id"],
            "client_secret": row["client_secret"],
            "scope": scope,
            "institution_code": "901",
        }
        for row, scope in ((card, "card"), (oob, "oob"))
    ]
    analyst_batches = [item.args[0] for item in bank.seed_analysts.await_args_list]
    assert [len(batch) for batch in analyst_batches] == [100, 1]
    assert [row for batch in analyst_batches for row in batch] == [
        {key: row[key] for key in ("analyst_no", "name", "password")} for row in analysts
    ]


@pytest.mark.parametrize(
    ("roster", "networks", "match"),
    [
        ([{"customer_no": f"C{10 + BATCH:08d}"}], [], "roster"),
        ([{"account_no": "99800000010"}], [], "roster"),
        ([{"phone": "01000000010"}], [], "roster"),
        ([], [{"_id": OOB_CLIENT_ID}], "credentials"),
    ],
)
async def test_existing_reservation_fails_before_any_bank_write(roster, networks, match):
    bank, truth = _bank(), _truth(roster=roster, networks=networks)
    with pytest.raises(ValueError, match=match):
        await stage6_seed.seed_world(bank, truth, 7, BATCH + 1, 10, set(), PREPARATION, FIRST)
    assert bank.mock_calls == []
    assert truth.roster_rows == roster
    assert truth.network_rows == networks


async def test_lost_seed_response_leaves_reservation_and_prevents_redispatch():
    bank, truth = _bank(), _truth()
    bank.seed_customers.side_effect = httpx.ReadTimeout("response lost")
    args = (bank, truth, 7, 2, 10, {"C00000010"}, PREPARATION, FIRST)
    with pytest.raises(httpx.ReadTimeout):
        await stage6_seed.seed_world(*args)
    assert [item[0] for item in bank.method_calls] == ["clock_set", "seed_customers"]
    with pytest.raises(ValueError, match="credentials"):
        await stage6_seed.seed_world(*args)
    bank.seed_customers.assert_awaited_once()
    bank.seed_accounts.assert_not_awaited()
    bank.seed_analysts.assert_not_awaited()
    assert not truth.roster_rows


async def test_same_world_seed_keeps_synthetic_customers_stable_on_separate_banks():
    first_bank, second_bank = _bank(), _bank()
    first_truth, second_truth = _truth(), _truth()
    for bank, truth in [(first_bank, first_truth), (second_bank, second_truth)]:
        await stage6_seed.seed_world(bank, truth, 7, 2, 10, set(), PREPARATION, FIRST)
        bank.seed_analysts.assert_not_awaited()
    assert first_truth.roster_rows == second_truth.roster_rows
    assert {row["client_secret"] for row in first_truth.network_rows}.isdisjoint(
        row["client_secret"] for row in second_truth.network_rows
    )
