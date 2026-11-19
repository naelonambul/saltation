"""Expand reserved Stage 6 episodes into synthetic bank operations."""

from saltation_gen.drivers.stage6_types import Episode, Step

_DAY = 86_400
PRIOR_DAY = {"A", "E", "F", "travel", "trip", "new_phone", "purchase"}
_TRAVEL = {"ip": "198.51.100.17", "region": "US-NY"}
_MULE = {"ip": "192.0.2.80", "device": "device-180"}
_HOUSEHOLD = {"ip": "192.0.2.55"}


def _cash(name: str, actor: str, offset: int, op: str, amount: int, label: str, **context: str) -> list[Step]:
    return [
        Step(f"{name}_start", "atm_start", actor, offset, label=label, context=context.copy()),
        Step(name, op, actor, offset, {"amount": amount}, label=label, context=context),
    ]


def _card(
    name: str,
    actor: str,
    offset: int,
    amount: int,
    merchant: str,
    label: str,
    *,
    country: str = "US",
    online: bool = False,
) -> Step:
    args = {
        "amount": amount,
        "currency": "KRW" if country == "KR" else "USD",
        "merchant_name": merchant,
        "mcc": "5411",
        "country": country,
        "entry_mode": "online" if online else "in_person",
    }
    if not online:
        latitude, longitude = ("37.566500", "126.978000") if country == "KR" else ("40.712800", "-74.006000")
        args.update(latitude=latitude, longitude=longitude)
    return Step(name, "card", actor, offset, args, label=label)


def _new_device(actor: str) -> dict[str, str]:
    return {"device": f"device-{actor}-2"}


def _takeover(actor: str, receiver: str) -> list[Step]:
    context = _TRAVEL | _new_device(actor)
    return [
        *[
            Step(f"bad_login_{index + 1}", "bad_login", actor, index * 10, label="A", context=context.copy())
            for index in range(3)
        ],
        Step("reset_request", "reset_request", actor, 30, label="A", context=context.copy()),
        Step("sms", "sms", actor, 31, label="A"),
        Step("reset_confirm", "reset_confirm", actor, 40, label="A", context=context.copy()),
        Step("login", "login", actor, 50, label="A", context=context.copy()),
        Step("payee", "payee", actor, 60, peer=receiver, label="A", context=context.copy()),
        Step(
            "transfer",
            "transfer",
            actor,
            75,
            {"amount": 1_500_000},
            peer=receiver,
            label="A",
            peer_label="normal",
            context=context,
        ),
    ]


def _victims_and_mule(roles: dict[str, str]) -> list[Step]:
    mule = roles["mule"]
    victims = ["victim_1"]
    if "victim_2" in roles:
        victims.append("victim_2")
    actions = [Step("mule_login", "login", mule, 295, label="C", context=_MULE.copy())]
    for index, role in enumerate(victims):
        victim = roles[role]
        offset = index * 30
        open_name = f"{role}_open"
        identity_name = f"{role}_identity"
        actions.extend(
            [
                Step(f"{role}_login", "login", victim, offset, label="normal"),
                Step(
                    open_name,
                    "term_open",
                    victim,
                    offset + 10,
                    {"amount": 1_000_000, "terms_id": 2},
                    label="normal",
                ),
                Step(
                    identity_name, "term_identity", victim, offset + 290, {"open_step": open_name}, label="B"
                ),
                Step(
                    f"{role}_close",
                    "term_close",
                    victim,
                    offset + 300,
                    {"open_step": open_name, "identity_step": identity_name},
                    label="B",
                ),
                Step(f"{role}_payee", "payee", victim, offset + 305, peer=mule, label="B", peer_label="C"),
                Step(
                    f"{role}_transfer",
                    "transfer",
                    victim,
                    offset + 310,
                    {"amount": 300_000},
                    peer=mule,
                    label="B",
                    peer_label="C",
                ),
                *_cash(f"{role}_withdraw", victim, offset + 315, "atm_withdraw", 700_000, "B"),
            ]
        )
    actions.extend(
        [
            Step(
                "mule_settlement",
                "inbound",
                mule,
                350,
                {"amount": 450_000, "sender_kind": "settlement"},
                label="C",
            ),
            Step(
                "mule_inbound",
                "inbound",
                mule,
                360,
                {"amount": 200_000, "sender_kind": "unrelated"},
                label="C",
            ),
            *_cash("mule_withdraw", mule, 370, "atm_withdraw", 900_000, "C"),
        ]
    )
    return actions


def _split_deposits(actor: str, second: str) -> list[Step]:
    actions = []
    for day in range(3):
        for index, customer in enumerate((actor, second), start=1):
            for deposit in (1, 2):
                name = f"deposit_{day + 1}_{index}_{deposit}"
                actions.extend(_cash(name, customer, day * _DAY, "atm_deposit", 4_800_000, "D"))
    return actions


def _foreign_card_misuse(actor: str) -> list[Step]:
    return [
        Step("login", "login", actor, label="G", context={"region": "KR-11"}),
        *_cash("withdraw", actor, 60, "atm_withdraw", 10_000, "G", region="KR-11"),
        _card("small_card", actor, 3_600, 5, "Harbor Market", "G"),
        _card("large_card", actor, 3_780, 20, "Cedar Grocery", "G"),
        _card("online_card", actor, 3_840, 20, "Union Food Hall", "G", online=True),
    ]


def _actor_steps(kind: str, actor: str, roles: dict[str, str]) -> list[Step]:
    match kind:
        case "A":
            return _takeover(actor, roles["receiver"])
        case "D":
            return _split_deposits(actor, roles["second"])
        case "E":
            return [
                Step("login", "login", actor, label="normal"),
                *[
                    Step(
                        f"transfer_{index + 1}",
                        "transfer",
                        actor,
                        60 + index * 15,
                        {"amount": amount},
                        peer=roles["receiver"],
                        label="E",
                        peer_label="normal",
                    )
                    for index, amount in enumerate((30_000, 30_000, 30_000, 240_000))
                ],
            ]
        case "F":
            return [Step("login", "login", actor, label="F")]
        case "G":
            return _foreign_card_misuse(actor)
        case "travel":
            return [Step("login", "login", actor, label="lookalike", context=_TRAVEL.copy())]
        case "trip":
            return [
                Step("login", "login", actor, 3_600, label="lookalike", context=_TRAVEL.copy()),
                _card("card", actor, 3_780, 20, "Maple Corner", "lookalike"),
            ]
        case "online":
            return [_card("card", actor, 3_840, 20, "Northside Foods", "lookalike", online=True)]
        case "new_phone":
            return [Step("login", "login", actor, label="lookalike", context=_new_device(actor))]
        case "household":
            return [
                Step("login", "login", actor, label="lookalike", context=_HOUSEHOLD.copy()),
                Step(
                    "second_login", "login", roles["second"], 60, label="lookalike", context=_HOUSEHOLD.copy()
                ),
            ]
        case "purchase" | "normal_transfer":
            label = "lookalike" if kind == "purchase" else "normal"
            return [
                Step("login", "login", actor, label="normal"),
                Step(
                    "transfer",
                    "transfer",
                    actor,
                    60,
                    {"amount": 240_000 if kind == "purchase" else 30_000},
                    peer=roles["receiver"],
                    label=label,
                    peer_label="normal",
                ),
            ]
        case "normal_income":
            sender_kind = roles.get("activity", "salary")
            amount = {"salary": 2_500_000, "settlement": 450_000, "unrelated": 200_000}[sender_kind]
            return [
                Step(
                    "inbound",
                    "inbound",
                    actor,
                    args={"amount": amount, "sender_kind": sender_kind},
                    label="normal",
                )
            ]
        case "normal_card":
            return [_card("card", actor, 0, 9_000, "River Daily Market", "normal", country="KR")]
        case "normal_atm":
            return _cash("withdraw", actor, 0, "atm_withdraw", 30_000, "normal")
        case "normal_login_balance":
            return [
                Step("login", "login", actor, label="normal"),
                Step("balance", "balance", actor, 10, label="normal"),
            ]
        case _:
            raise ValueError(f"unknown Stage 6 episode kind: {kind}")


def steps(episode: Episode) -> list[Step]:
    """Return the episode's steps in execution order, relative to its start."""
    roles = episode.roles
    if episode.kind == "BC":
        actions = _victims_and_mule(roles)
    else:
        actor = roles["actor"]
        actions = _actor_steps(episode.kind, actor, roles)
        if episode.kind in PRIOR_DAY:
            setup_at = -_DAY
            if episode.kind == "F":
                setup_at = -41 * 3600 if episode.window == "live" else -17 * 3600
            actions.append(Step("home_baseline", "login", actor, setup_at, label="normal"))
            if episode.kind == "E":
                actions.append(
                    Step(
                        "baseline_transfer",
                        "transfer",
                        actor,
                        setup_at + 60,
                        {"amount": 30_000},
                        peer=roles["receiver"],
                        label="normal",
                        peer_label="normal",
                    )
                )
    return sorted(actions, key=lambda action: action.offset)
