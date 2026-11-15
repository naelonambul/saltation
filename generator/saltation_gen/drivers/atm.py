"""Read-only roster smoke flow; ATM labels belong to stage 6."""

from dataclasses import replace
from typing import Any

from saltation_gen.client.bank import BankClient, BankError, atm_idempotency_key, new_ulid
from saltation_gen.config import ATM_TERMINALS
from saltation_gen.personas.normal import home_context
from saltation_gen.truth import TruthStore


async def smoke(bank: BankClient, truth: TruthStore) -> dict[str, Any]:
    # The seed interface creates DEMAND accounts; older roster entries omit account_type.
    roster = sorted(await truth.load_roster(), key=lambda c: c["customer_no"])
    customer = next(
        (c for c in roster if c.get("atm_pin") and c.get("account_type", "DEMAND") == "DEMAND"),
        None,
    )
    if customer is None:
        raise RuntimeError("seed a demand account with an ATM PIN first")
    terminal = ATM_TERMINALS[0]
    ctx = replace(home_context(customer), region=terminal["region"])
    requests = {}

    async def checked(step, call, *args, expected=200):
        rid = requests[step] = new_ulid()
        try:
            status, body = await call(rid, *args, ctx)
        except BankError as error:
            raise RuntimeError(f"{error.status} {error.body.get('rsp_code')}") from None
        if status != expected:
            raise RuntimeError(f"{status} {body.get('rsp_code')}")
        return body

    args = (terminal["terminal_id"], customer["account_no"])
    pin = customer["atm_pin"]
    wrong = f"{(int(pin) + 1) % 10000:04d}"
    await checked("wrong_pin", bank.atm_start_session, *args, wrong, expected=422)
    body = await checked("start", bank.atm_start_session, *args, pin)
    token = body["atm_session_token"]
    await checked("withdraw", bank.atm_withdraw, token, atm_idempotency_key(), 10000)
    await checked("deposit", bank.atm_deposit, token, atm_idempotency_key(), 20000)
    await checked("balance", bank.atm_balance, token)
    await checked("restart", bank.atm_start_session, *args, pin)
    return {
        **{k: terminal[k] for k in ("terminal_id", "region", "latitude", "longitude")},
        "account_no": customer["account_no"],
        "requests": requests,
    }
