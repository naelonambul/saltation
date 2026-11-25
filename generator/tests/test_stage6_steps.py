from collections import Counter
from datetime import datetime, timedelta, timezone

import pytest

from saltation_gen.config.card import card_amount_krw
from saltation_gen.drivers.stage6_steps import PRIOR_DAY, steps
from saltation_gen.drivers.stage6_types import Episode, Step

_KST = timezone(timedelta(hours=9))
_ACTOR, _RECEIVER, _SECOND = "C00000001", "C00000002", "C00000003"


def _episode(kind: str, **roles: str) -> Episode:
    return Episode("fixture", kind, datetime(2026, 1, 1, 23, 59, 59, tzinfo=_KST), roles, "fixture")


@pytest.mark.parametrize(
    ("kind", "order", "label"),
    [
        (
            "A",
            [
                "bad_login",
                "bad_login",
                "bad_login",
                "reset_request",
                "sms",
                "reset_confirm",
                "login",
                "payee",
                "transfer",
            ],
            "A",
        ),
        ("D", ["atm_start", "atm_deposit"] * 12, "D"),
        ("E", ["login", "transfer", "transfer", "transfer", "transfer"], "E"),
        ("F", ["login"], "F"),
        ("G", ["login", "atm_start", "atm_withdraw", "card", "card", "card", "card"], "G"),
        ("travel", ["login"], "lookalike"),
        ("trip", ["login", "card"], "lookalike"),
        ("online", ["card"], "lookalike"),
        ("new_phone", ["login"], "lookalike"),
        ("purchase", ["login", "transfer"], "lookalike"),
        ("household", ["login", "login"], "lookalike"),
        ("normal_transfer", ["login", "transfer"], "normal"),
        ("normal_income", ["inbound"], "normal"),
        ("normal_card", ["card"], "normal"),
        ("normal_atm", ["atm_start", "atm_withdraw"], "normal"),
        ("normal_login_balance", ["login", "balance"], "normal"),
    ],
)
def test_fixture_order(kind, order, label):
    episode = _episode(kind, actor=_ACTOR, receiver=_RECEIVER, second=_SECOND)
    actions = steps(episode)
    assert actions == steps(episode)
    workload = [action for action in actions if action.offset >= 0]
    assert [action.op for action in workload] == order
    prep = [action for action in actions if action.offset < 0]
    assert bool(prep) == (kind in PRIOR_DAY)
    assert all(action.label == "normal" for action in prep)
    assert [action.offset for action in actions] == sorted(action.offset for action in actions)
    assert len({action.name for action in actions}) == len(actions)
    assert all(action.actor in episode.roles.values() for action in actions)
    assert all(
        action.label == label or action.op == "login" and action.label == "normal" for action in workload
    )
    for action in actions:
        if action.op in {"transfer", "payee"}:
            assert action.peer == _RECEIVER
        if "amount" in action.args:
            assert type(action.args["amount"]) is int and action.args["amount"] > 0
    _assert_atm_pairs(actions)


def _assert_atm_pairs(actions: list[Step]) -> None:
    starts = [action for action in actions if action.op == "atm_start"]
    cash = [index for index, action in enumerate(actions) if action.op in {"atm_withdraw", "atm_deposit"}]
    assert len(starts) == len(cash)
    for index in cash:
        assert index > 0
        start, action = actions[index - 1 : index + 1]
        assert (start.op, start.actor, start.label) == ("atm_start", action.actor, action.label)
        assert 0 <= action.offset - start.offset < 180


def test_takeover_and_behavior_lookalikes():
    ordinary, *takeover = steps(_episode("A", actor=_ACTOR, receiver=_RECEIVER))
    assert (ordinary.op, ordinary.context, ordinary.offset) == ("login", {}, -86_400)
    assert [action.offset for action in takeover] == [0, 10, 20, 30, 31, 40, 50, 60, 75]
    assert takeover[0].context == takeover[6].context
    assert takeover[6].context["device"] != f"dev-{_ACTOR}"
    assert takeover[-1].args["amount"] == 1_500_000
    assert all(not action.args for action in takeover[:-1])

    household = steps(_episode("household", actor=_ACTOR, second=_SECOND))
    assert [action.actor for action in household] == [_ACTOR, _SECOND]
    assert household[0].context["ip"] == household[1].context["ip"]
    assert all("device" not in action.context for action in household)
    assert steps(_episode("new_phone", actor=_ACTOR))[-1].context["device"]
    assert steps(_episode("travel", actor=_ACTOR))[-1].context["region"].startswith("US")
    first, late = steps(_episode("F", actor=_ACTOR))
    assert first.context == late.context == {}
    assert late.offset - first.offset == 17 * 3600


@pytest.mark.parametrize("two_victims", [False, True])
def test_bc_links_each_victim_to_the_mule(two_victims):
    roles = {"victim_1": _ACTOR, "mule": _RECEIVER}
    if two_victims:
        roles["victim_2"] = _SECOND
    actions = steps(_episode("BC", **roles))
    first_victim = [
        "victim_1_login",
        "victim_1_open",
        "victim_1_identity",
        "mule_login",
        "victim_1_close",
        "victim_1_payee",
        "victim_1_transfer",
        "victim_1_withdraw_start",
        "victim_1_withdraw",
    ]
    if two_victims:
        first_victim[2:2] = ["victim_2_login", "victim_2_open"]
        first_victim.extend(
            [
                "victim_2_identity",
                "victim_2_close",
                "victim_2_payee",
                "victim_2_transfer",
                "victim_2_withdraw_start",
                "victim_2_withdraw",
            ]
        )
    assert [action.name for action in actions] == first_victim + [
        "mule_settlement",
        "mule_inbound",
        "mule_withdraw_start",
        "mule_withdraw",
    ]

    for victim in (roles[key] for key in roles if key.startswith("victim_")):
        login, opened, identity, closed, payee, transfer, _, withdrawal = [
            action for action in actions if action.actor == victim
        ]
        assert not login.context
        assert (opened.op, opened.label, opened.args["terms_id"]) == ("term_open", "normal", 2)
        assert (closed.op, closed.label, closed.args["open_step"]) == ("term_close", "B", opened.name)
        assert (identity.op, identity.args["open_step"]) == ("term_identity", opened.name)
        assert closed.args["identity_step"] == identity.name
        assert opened.offset < identity.offset < closed.offset
        for outbound in (payee, transfer):
            assert (outbound.peer, outbound.label, outbound.peer_label) == (_RECEIVER, "B", "C")
        assert 500_000 <= withdrawal.args["amount"] <= 1_000_000

    mule_actions = [action for action in actions if action.actor == _RECEIVER]
    login, settlement, inbound, _, withdrawal = mule_actions
    assert all(action.label == "C" for action in mule_actions)
    assert {settlement.args["sender_kind"], inbound.args["sender_kind"]} == {"settlement", "unrelated"}
    assert 500_000 <= withdrawal.args["amount"] <= 1_000_000
    another_mule = steps(_episode("BC", victim_1=_ACTOR, mule="C00000004"))
    login_elsewhere = next(action for action in another_mule if action.name == login.name)
    assert login.context == login_elsewhere.context
    assert login.context["device"] and login.context["ip"]
    _assert_atm_pairs(actions)


def test_d_splits_totals_across_accounts_and_business_dates():
    episode = _episode("D", actor=_ACTOR, second=_SECOND)
    actions = steps(episode)
    count = Counter()
    total = Counter()
    for action in actions:
        if action.op == "atm_deposit":
            at = episode.start + timedelta(seconds=action.offset)
            key = action.actor, at.date()
            count[key] += 1
            total[key] += action.args["amount"]
    assert {action.offset for action in actions} == {0, 86_400, 172_800}
    expected = {
        (actor, (episode.start + timedelta(days=day)).date())
        for actor in (_ACTOR, _SECOND)
        for day in range(3)
    }
    assert count.keys() == expected
    assert all(value >= 2 for value in count.values())
    assert total == dict.fromkeys(expected, 9_600_000)


def test_g_card_modes_match_benign_payments():
    episode = _episode("G", actor=_ACTOR)
    login, start, withdrawal, small, in_person, online, declined = steps(episode)
    assert all(action.context["region"].startswith("KR-") for action in (login, start, withdrawal))
    assert withdrawal.offset < small.offset < in_person.offset < online.offset < declined.offset
    assert small.args["amount"] < in_person.args["amount"] == online.args["amount"] < declined.args["amount"]
    assert declined.args.get("expect_declined") is True

    for card, lookalike_kind, mode in [(in_person, "trip", "in_person"), (online, "online", "online")]:
        lookalike = steps(_episode(lookalike_kind, actor=_RECEIVER))[-1]
        assert card.offset == lookalike.offset
        assert {key: card.args[key] for key in ("country", "currency", "amount", "entry_mode")} == {
            key: lookalike.args[key] for key in ("country", "currency", "amount", "entry_mode")
        }
        assert card.args["entry_mode"] == mode

    for card in (small, in_person, online, steps(_episode("normal_card", actor=_ACTOR))[0]):
        args = card.args
        assert args["merchant_name"] and args["mcc"] == "5411"
        assert card_amount_krw(args["country"], args["currency"], args["amount"]) > 0
        assert ("latitude" in args) == ("longitude" in args) == (args["entry_mode"] == "in_person")


@pytest.mark.parametrize("sender_kind", [None, "salary", "settlement", "unrelated"])
def test_normal_income_sender_kind(sender_kind):
    roles = {"actor": _ACTOR}
    if sender_kind is not None:
        roles["activity"] = sender_kind
    (inbound,) = steps(_episode("normal_income", **roles))
    assert inbound.args["sender_kind"] == (sender_kind or "salary")
