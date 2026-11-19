"""Run the short card proof and hand credential-free evidence to Rust."""

import asyncio
import json
from datetime import date

import httpx

from saltation_gen.client.bank import BankClient, new_ulid
from saltation_gen.config import Settings
from saltation_gen.drivers import cards
from saltation_gen.drivers.card_plan import proof_time
from saltation_gen.drivers.seed import seed_card_fixture
from saltation_gen.truth import TruthStore

FIXTURE_DATE = date(2026, 11, 1)
EXPECTED_SEQUENCE = (
    "g_login",
    "g_atm_start",
    "g_atm_withdraw",
    "trip_login",
    "network_token",
    "g_small",
    "g_large",
    "g_decline",
    "trip_card",
    "online_card",
    "normal_card",
)
EXPECTED_PATHS = (
    "/v2.0/ext/auth/login",
    "/atm/v1/session",
    "/atm/v1/cash/withdraw",
    "/v2.0/ext/auth/login",
    "/oauth/2.0/token",
    *("/card/v1/authorize",) * 6,
)


class EvidenceTransport(httpx.AsyncHTTPTransport):
    def __init__(self):
        super().__init__()
        self.calls = []
        self.normal_amount = None

    async def handle_async_request(self, request):
        path = request.url.path
        if path in EXPECTED_PATHS:
            self.calls.append((path, request.headers["X-Request-Id"]))
            if len(self.calls) == len(EXPECTED_SEQUENCE) and path == EXPECTED_PATHS[-1]:
                self.normal_amount = int(json.loads(request.content)["amount"])
        return await super().handle_async_request(request)


async def main():
    settings = Settings.from_env()
    transport = EvidenceTransport()
    bank = BankClient(settings.bank_url, transport=transport)
    truth = TruthStore(settings.truth_uri)
    run_id = new_ulid()
    try:
        await truth.setup()
        if (await bank.clock())["mode"] != "BACKFILL":
            raise RuntimeError("the card fixture requires a fresh backfill schema")
        await bank.clock_set(proof_time(FIXTURE_DATE, 8, 59).isoformat())
        # The integration script just reset Oracle. Old runs of this dedicated fixture
        # can no longer resume against it. Match their labels by run ID only.
        previous = {"kind": "generator-cards", "inputs.version": cards.VERSION}
        async for old in truth.runs.find(previous, {"_id": 1}):
            await truth.labels.delete_many({"run_id": old["_id"]})
            await truth.runs.delete_one(previous | {"_id": old["_id"]})
        await seed_card_fixture(bank, truth, settings.seed)
        result = await cards.run(bank, truth, run_id, settings.seed, FIXTURE_DATE)
        assert result["attempts"] == {"attempted": 10, "completed": 9, "declined": 1, "unresolved": 0}
        assert result["instances"] == {"G": 1, "lookalike": 2, "normal": 1}
        assert await cards.run(bank, truth, run_id, settings.seed, FIXTURE_DATE) == result
        paths = tuple(call[0] for call in transport.calls)
        assert paths == EXPECTED_PATHS, paths
        ids = [call[1] for call in transport.calls]
        assert len(set(ids)) == len(EXPECTED_SEQUENCE)
        requests = dict(zip(EXPECTED_SEQUENCE, ids, strict=True))
        assert all(requests[name] == request_id for name, request_id in result["requests"].items())
        fixture = {
            "run_id": run_id,
            "subjects": {
                "g": {"account_no": "99897100000", "customer_no": "C97100000"},
                "trip": {"account_no": "99897100001", "customer_no": "C97100001"},
                "online": {"account_no": "99897100002", "customer_no": "C97100002"},
                "normal": {"account_no": "99897100003", "customer_no": "C97100003"},
            },
            "requests": requests,
            "normal_amount": transport.normal_amount,
            "date": FIXTURE_DATE.isoformat(),
        }
        print(json.dumps(fixture))
    finally:
        await bank.close()
        await truth.close()


if __name__ == "__main__":
    asyncio.run(main())
