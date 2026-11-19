from datetime import datetime
from unittest.mock import MagicMock

import httpx
import pytest

from saltation_gen.client.bank import BankClient
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6_execute import Stage6Executor
from saltation_gen.drivers.stage6_steps import steps
from saltation_gen.drivers.stage6_types import Episode
from saltation_gen.truth.recovery import RecoveryError

NOW = datetime(2026, 1, 5, 10, tzinfo=KST)
PAYER = "99800000001"
TERM = "20000000000016"
ACTOR = "C00000001"


class Journal:
    run_id = "fixture"
    owner = "runner"

    def __init__(self):
        self.rows = {}
        self.labels = {}

    async def get_operation(self, episode_id, name):
        return self.rows.get((episode_id, name))

    async def prepare(self, episode_id, name, record):
        self.rows[episode_id, name] = record
        return record

    async def transition(self, episode_id, name, old, new, result=None):
        row = self.rows[episode_id, name]
        assert row["state"] == old
        row["state"] = new
        if result is not None:
            row["result"] = result

    async def ensure_labels(self, rid, labels):
        self.labels[rid] = labels


@pytest.mark.parametrize(
    "fault", [None, "denied", "timeout", "wrong_payer", "masked", "wrong_id", "wrong_rid"]
)
async def test_labels_exist_at_http_boundary_and_invalid_identity_cannot_close(fault):
    journal = Journal()
    episode = Episode("test", "BC", NOW, {"victim_1": ACTOR, "mule": "C00000002"}, "annual")
    selected = {step.op: step for step in steps(episode) if step.actor == ACTOR}
    seen = []

    def respond(request):
        seen.append(request.url.path)
        rid = request.headers["X-Request-Id"]
        labels = journal.labels[rid]
        subjects = {(label["subject_type"], label["subject_id"]) for label in labels}
        payload = {"rsp_code": "A0000", "request_id": rid, "deposit_id": "16"}
        if request.url.path.endswith("/identity"):
            assert request.method == "GET"
            assert request.headers["Authorization"] == "Bearer synthetic-session"
            assert subjects == {("customer", ACTOR)}
            row = journal.rows[episode.id, selected["term_identity"].name]
            original = journal.rows[episode.id, selected["term_open"].name]
            assert row["state"] == "unconfirmed"
            assert row["dependency"] == {
                "opening_request_id": original["request_id"],
                "deposit_id": "16",
                "account_num_masked": TERM[:-3] + "***",
                "payer_account_no": PAYER,
            }
            if fault == "timeout":
                raise httpx.ReadTimeout("synthetic response loss")
            if fault == "denied":
                return httpx.Response(404, json={"rsp_code": "A0003"})
            return httpx.Response(
                200,
                json=payload
                | {
                    "request_id": "other" if fault == "wrong_rid" else rid,
                    "deposit_id": "17" if fault == "wrong_id" else "16",
                    "term_account_no": TERM[:-3] + "***" if fault == "masked" else TERM,
                    "linked_account_no": "99800000002" if fault == "wrong_payer" else PAYER,
                },
            )
        assert request.method == "POST"
        if request.url.path.endswith("/close"):
            assert subjects == {("account", TERM), ("account", PAYER)}
            assert all(label["label"] == "B" for label in labels)
            query = journal.rows[episode.id, selected["term_identity"].name]
            assert query["state"] == "succeeded"
            return httpx.Response(200, json=payload)
        assert subjects == {("account", PAYER)}
        return httpx.Response(
            200, json=payload | {"account_num_masked": TERM[:-3] + "***", "linked_fintech_use_num": "4001"}
        )

    bank = BankClient("http://synthetic", transport=httpx.MockTransport(respond))
    truth = MagicMock()
    executor = Stage6Executor(
        bank, truth, journal, {ACTOR: {"customer_no": ACTOR, "account_no": PAYER, "fintech_use_num": "4001"}}
    )
    executor.truth_labels = journal
    executor.customer_tokens[ACTOR] = ("synthetic-session", NOW, executor._context(selected["term_open"]))
    try:
        await executor.execute(episode, selected["term_open"], NOW)
        if fault:
            with pytest.raises(RecoveryError, match="unconfirmed outcome"):
                await executor.execute(episode, selected["term_identity"], NOW)
            with pytest.raises(RecoveryError, match="unresolved"):
                await executor.execute(episode, selected["term_identity"], NOW)
            with pytest.raises(RecoveryError, match="requires a verified identity"):
                await executor.execute(episode, selected["term_close"], NOW)
            assert seen == ["/v2.0/ext/deposits", "/internal/v1/deposits/16/identity"]
        else:
            await executor.execute(episode, selected["term_identity"], NOW)
            await executor.execute(episode, selected["term_close"], NOW)
            assert seen[-1] == "/v2.0/ext/deposits/16/close"
    finally:
        await bank.close()
