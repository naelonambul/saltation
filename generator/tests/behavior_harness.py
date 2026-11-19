"""Drive the behavior proof and hand the transport-observed IDs to Rust."""

import asyncio
import json
from datetime import date

import httpx

from saltation_gen.client.bank import BankClient, new_ulid
from saltation_gen.config import Settings
from saltation_gen.drivers import behavior
from saltation_gen.drivers.behavior_plan import VERSION, instant
from saltation_gen.truth import TruthStore

FIXTURE_DATE = date(2026, 11, 1)
LOGIN = "/v2.0/ext/auth/login"
TRANSFER = "/v2.0/transfer/withdraw/fin_num"
EXPECTED = (
    ("e_baseline_login", LOGIN),
    ("e_baseline_transfer", TRANSFER),
    ("f_baseline", LOGIN),
    ("travel_baseline", LOGIN),
    ("phone_baseline", LOGIN),
    ("buyer_baseline", LOGIN),
    ("f_anomaly", LOGIN),
    ("e_setup", LOGIN),
    ("e_1", TRANSFER),
    ("e_2", TRANSFER),
    ("e_3", TRANSFER),
    ("e_4", TRANSFER),
    ("buyer_setup", LOGIN),
    ("purchase", TRANSFER),
    ("travel_login", LOGIN),
    ("phone_login", LOGIN),
    ("household_1", LOGIN),
    ("household_2", LOGIN),
)


class EvidenceTransport(httpx.AsyncHTTPTransport):
    def __init__(self):
        super().__init__()
        self.calls = []

    async def handle_async_request(self, request):
        if request.url.path in (LOGIN, TRANSFER):
            self.calls.append((request.url.path, request.headers["X-Request-Id"]))
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
            raise RuntimeError("the behavior fixture requires a fresh backfill schema")
        await bank.clock_set(instant(FIXTURE_DATE, 9, 58).isoformat())
        previous = {"kind": "generator-behavior", "inputs.version": VERSION}
        async for old in truth.runs.find(previous, {"_id": 1}):
            await truth.labels.delete_many({"run_id": old["_id"]})
            await truth.runs.delete_one(previous | {"_id": old["_id"]})
        await behavior.seed_fixture(bank, truth, settings.seed)
        result = await behavior.run(bank, truth, run_id, settings.seed, FIXTURE_DATE)
        assert result["attempted"] == result["completed"] == len(EXPECTED)
        assert result["unresolved"] == result["recovery_logins"] == 0
        assert result["instances"] == {"E": 1, "F": 1, "lookalike": 4}
        assert await behavior.run(bank, truth, run_id, settings.seed, FIXTURE_DATE) == result
        requests = {}
        for (name, expected_path), (actual_path, request_id) in zip(EXPECTED, transport.calls, strict=True):
            assert expected_path == actual_path
            requests[name] = request_id
        assert len(set(requests.values())) == len(EXPECTED)
        assert requests == result["requests"]
        print(
            json.dumps(
                {
                    "run_id": run_id,
                    "requests": requests,
                    "date": FIXTURE_DATE.isoformat(),
                    "recovery_logins": 0,
                }
            )
        )
    finally:
        await bank.close()
        await truth.close()


if __name__ == "__main__":
    asyncio.run(main())
