from dataclasses import dataclass


@dataclass(frozen=True)
class OperationSpec:
    spec: str
    method: str
    path: str
    statuses: tuple[int, ...]


OPERATIONS: dict[str, OperationSpec] = {
    "seed_customers": OperationSpec("internal", "POST", "/internal/v1/seed/customers", (200,)),
    "seed_accounts": OperationSpec("internal", "POST", "/internal/v1/seed/accounts", (200,)),
    "seed_cards": OperationSpec("internal", "POST", "/internal/v1/seed/cards", (200,)),
    "seed_analysts": OperationSpec("internal", "POST", "/internal/v1/seed/analysts", (200,)),
    "outbox_backlog": OperationSpec("internal", "GET", "/internal/v1/outbox", (200,)),
    "login": OperationSpec("public", "POST", "/v2.0/ext/auth/login", (200,)),
    "balance": OperationSpec("public", "GET", "/v2.0/account/balance/fin_num", (200, 404)),
    "transfer": OperationSpec("public", "POST", "/v2.0/transfer/withdraw/fin_num", (200, 422)),
    "seed_counterparties": OperationSpec("internal", "POST", "/internal/v1/seed/counterparties", (200,)),
    "seed_counterparty_clients": OperationSpec(
        "internal", "POST", "/internal/v1/seed/counterparty_clients", (200,)
    ),
    "clock": OperationSpec("internal", "GET", "/internal/v1/clock", (200,)),
    "clock_set": OperationSpec("internal", "POST", "/internal/v1/clock/set", (200,)),
    "clock_advance": OperationSpec("internal", "POST", "/internal/v1/clock/advance", (200,)),
    "go_live": OperationSpec("internal", "POST", "/internal/v1/mode/live", (200,)),
    "sms": OperationSpec("internal", "GET", "/internal/v1/sms/{phone}", (200, 404)),
    "client_token": OperationSpec("inbound", "POST", "/oauth/2.0/token", (200,)),
    "card_authorize": OperationSpec("card", "POST", "/card/v1/authorize", (200, 422)),
    "inbound_deposit": OperationSpec("inbound", "POST", "/v2.0/transfer/deposit/fin_num", (200, 422)),
    "inquiry_receive": OperationSpec("public", "POST", "/v2.0/inquiry/receive", (200, 422)),
    "register_payee": OperationSpec("public", "POST", "/v2.0/ext/payees", (200, 409, 422)),
    "delete_payee": OperationSpec("public", "DELETE", "/v2.0/ext/payees", (200, 404)),
    "password_reset_request": OperationSpec(
        "public", "POST", "/v2.0/ext/auth/password_reset/request", (200,)
    ),
    "password_reset_confirm": OperationSpec(
        "public", "POST", "/v2.0/ext/auth/password_reset/confirm", (200, 401)
    ),
    "payees": OperationSpec("public", "GET", "/v2.0/ext/payees", (200,)),
    "products": OperationSpec("public", "GET", "/v2.0/ext/products", (200,)),
    "deposit": OperationSpec("public", "GET", "/v2.0/ext/deposits/{id}", (200,)),
    "deposit_identity": OperationSpec(
        "internal", "GET", "/internal/v1/deposits/{deposit_id}/identity", (200,)
    ),
    "loan": OperationSpec("public", "GET", "/v2.0/ext/loans/{id}", (200,)),
    "open_deposit": OperationSpec("public", "POST", "/v2.0/ext/deposits", (200, 404, 422)),
    "take_loan": OperationSpec("public", "POST", "/v2.0/ext/loans", (200, 404, 422)),
    "close_deposit": OperationSpec("public", "POST", "/v2.0/ext/deposits/{id}/close", (200, 404, 422)),
    "repay_loan": OperationSpec("public", "POST", "/v2.0/ext/loans/{id}/repay", (200, 404, 422)),
    "seed_atm_terminals": OperationSpec("internal", "POST", "/internal/v1/seed/atm_terminals", (200,)),
    "atm_start_session": OperationSpec("atm", "POST", "/atm/v1/session", (200, 400, 404, 422, 503)),
    "atm_end_session": OperationSpec("atm", "POST", "/atm/v1/session/end", (200, 400, 401, 503)),
    "atm_withdraw": OperationSpec("atm", "POST", "/atm/v1/cash/withdraw", (200, 400, 401, 409, 422, 503)),
    "atm_deposit": OperationSpec("atm", "POST", "/atm/v1/cash/deposit", (200, 400, 401, 409, 422, 503)),
    "atm_balance": OperationSpec("atm", "GET", "/atm/v1/balance", (200, 400, 401, 422, 503)),
}


def operation_path(name: str, **params: str) -> str:
    template = OPERATIONS[name].path
    if params:
        return template.format(**params)
    if "{" in template or "}" in template:
        raise ValueError(f"{name} requires path parameters: {template}")
    return template
