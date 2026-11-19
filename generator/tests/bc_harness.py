"""Drive the real lost-response proof in two isolated generator containers."""

import json
import os
import sys

TRANSFER = "/v2.0/transfer/withdraw/fin_num"
OPERATIONS = {
    "/v2.0/ext/auth/login": "login",
    TRANSFER: "transfer",
    "/atm/v1/session": "atm_start",
    "/atm/v1/cash/withdraw": "withdraw",
}
FAULT_EXIT = 75
TRANSFER_AMOUNT = 50_000
WITHDRAW_AMOUNT = 10_000
FIXTURE_START = 97_000_000


def emit(**record):
    print(json.dumps(record), flush=True)


async def run(phase, run_id, owner):
    import hashlib

    import httpx

    from saltation_gen.client.bank import BankClient
    from saltation_gen.config import Settings
    from saltation_gen.drivers import bc
    from saltation_gen.drivers.seed import make_customers, seed, seed_terminals
    from saltation_gen.truth import TruthStore

    class Transport(httpx.AsyncHTTPTransport):
        async def handle_async_request(self, request):
            path = request.url.path
            operation = OPERATIONS.get(path)
            if operation:
                request_id = request.headers["X-Request-Id"]
                record = {"operation": operation, "request_id": request_id}
                if path == TRANSFER:
                    record["fingerprint"] = hashlib.sha256(request.content).hexdigest()
                emit(**record)
            response = await super().handle_async_request(request)
            if phase == "fault" and path == TRANSFER:
                await response.aread()
                body = response.json()
                if response.status_code != 200 or (body.get("rsp_code"), body.get("bank_rsp_code")) != (
                    "A0000",
                    "000",
                ):
                    raise RuntimeError(
                        f"fault requires a successful bank transfer, got {response.status_code}"
                    )
                emit(fault="discarded_committed_transfer_response", owner=owner)
                os._exit(FAULT_EXIT)
            return response

    settings = Settings.from_env()
    bank = BankClient(settings.bank_url, transport=Transport())
    truth = TruthStore(settings.truth_uri)
    sender, receiver = make_customers(settings.seed, 2, FIXTURE_START)
    try:
        await truth.setup()
        if phase == "fault":
            assert (await bank.clock())["mode"] == "BACKFILL"
            await truth.roster.delete_many(
                {"customer_no": {"$in": [sender["customer_no"], receiver["customer_no"]]}}
            )
            await seed(bank, truth, settings.seed, 2, FIXTURE_START)
            await seed_terminals(bank)
            emit(
                fixture={
                    "run_id": run_id,
                    "sender_account": sender["account_no"],
                    "receiver_account": receiver["account_no"],
                    "sender_customer": sender["customer_no"],
                    "receiver_customer": receiver["customer_no"],
                    "sender_opening": sender["opening_balance"],
                    "receiver_opening": receiver["opening_balance"],
                    "transfer_amount": TRANSFER_AMOUNT,
                    "withdraw_amount": WITHDRAW_AMOUNT,
                }
            )
        args = (
            bank,
            truth,
            run_id,
            sender["account_no"],
            receiver["account_no"],
            TRANSFER_AMOUNT,
            WITHDRAW_AMOUNT,
        )
        result = await bc.run(
            *args,
            owner=owner if phase == "fault" else None,
            exited_owner=owner if phase == "resume" else None,
        )
        if phase == "fault":
            raise RuntimeError("the fault was not injected")
        assert await bc.run(*args) == result
        emit(completed=run_id)
    finally:
        await bank.close()
        await truth.close()


def combine(fault_path, resume_path, owner):
    from pathlib import Path

    def read(path):
        return [json.loads(line) for line in Path(path).read_text().splitlines()]

    fault, resume = read(fault_path), read(resume_path)
    fixture = fault[0]["fixture"]
    assert [row["operation"] for row in fault[1:-1]] == ["login", "transfer"]
    assert fault[-1] == {"fault": "discarded_committed_transfer_response", "owner": owner}
    assert [row["operation"] for row in resume[:-1]] == ["login", "transfer", "atm_start", "withdraw"]
    assert resume[-1] == {"completed": fixture["run_id"]}
    assert resume[1] == fault[2], "transfer body or request ID changed on replay"
    ids = [fault[1]["request_id"], *(row["request_id"] for row in resume[:-1])]
    names = ("original_login_id", "recovery_login_id", "transfer_id", "atm_start_id", "withdraw_id")
    assert len(ids) == len(names) == len(set(ids))
    fixture.update({name: ids[i] for i, name in enumerate(names)})
    print(json.dumps(fixture))


if __name__ == "__main__":
    phase, *args = sys.argv[1:]
    if phase == "combine":
        combine(*args)
    else:
        import asyncio

        asyncio.run(run(phase, *args))
