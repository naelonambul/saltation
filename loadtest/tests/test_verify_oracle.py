from contextlib import asynccontextmanager
from datetime import date, timedelta

import pytest

from saltation_loadtest import verify_oracle as oracle

DAY = date(2026, 11, 20)
NEXT = DAY + timedelta(days=1)
STAMP = "2026-11-20 00:00:00.000000"
START = [
    (1, "sender", 101, 1000, 2, "ACTIVE", DAY, 0, 0, None, None, 0),
    (2, "receiver", 102, 200, 2, "ACTIVE", DAY, 0, 0, None, None, 0),
]
SEED = [
    (1, DAY, 1, 1000, 1000, None, 10, "SYSTEM"),
    (2, DAY, 1, 200, 200, None, 11, "SYSTEM"),
]
ATTEMPT = {
    "request_id": "request1",
    "bank_tran_id": "bank1",
    "sender": "sender",
    "receiver": "receiver",
    "amount": 100,
    "status": 200,
    "succeeded": True,
    "business_rejection": None,
}
ZERO = {"daily_close_mismatches": 0, "duplicate_movements": 0, "f21_violations": 0}


def account(row, balance, next_sequence):
    return (*row[:3], balance, next_sequence, *row[5:])


def clock(day, *, pending=0):
    return [(day, day - timedelta(days=1), pending, STAMP)]


def request(request_id="request1", status="DONE", *, day=DAY, fresh=1):
    return (request_id, "bank1", "C:101", "bank1", "TRANSFER", status, day, fresh)


def postings(day=DAY, *, sequence=2, transfer=20, request_id="request1", amount=100):
    already_moved = 100 * (sequence - 2)
    return [
        (1, day, sequence, -amount, 1000 - already_moved - amount, request_id, transfer, "MOBILE"),
        (2, day, sequence, amount, 200 + already_moved + amount, request_id, transfer, "MOBILE"),
    ]


def movement(rows, *, amount=100):
    return [
        (transfer, request_id, "bank1", "transfer", amount, account_id, seq, signed, request_id, day)
        for account_id, day, seq, signed, _, request_id, transfer, _ in rows
    ]


def post_snapshot(*, day=DAY, rows=None):
    rows = postings(day) if rows is None else rows
    return {
        oracle._CLOCK: clock(day),
        oracle._SCOPE: [account(START[0], 900, 3), account(START[1], 300, 3)],
        oracle._LEDGER: sorted(SEED + rows, key=lambda row: (row[0], row[1], row[2])),
        oracle._REQUESTS: [request(day=day)],
        oracle._MOVEMENTS: movement(rows),
        oracle._UNLINKED: [],
        oracle._FINISHED: [(DAY,)],
        oracle._CLOSES: [(1, DAY, 0, 1000, "Y"), (2, DAY, 0, 200, "Y")],
    }


@pytest.fixture
def snapshots(monkeypatch):
    for name in oracle._ENV:
        monkeypatch.setenv(name, "operator-supplied")
    baseline = {oracle._CLOCK: clock(DAY), oracle._SCOPE: START, oracle._LEDGER: SEED}
    after = post_snapshot()
    phases = []

    @asynccontextmanager
    async def read_snapshot(settings, phase):
        assert settings == ("operator-supplied",) * 3
        phases.append(phase)
        yield {"baseline": baseline, "post-scenario": after, "daily-close": after}[phase]

    async def rows(cursor, sql, **kwargs):
        return cursor[sql]

    monkeypatch.setattr(oracle, "_snapshot", read_snapshot)
    monkeypatch.setattr(oracle, "_rows", rows)
    return baseline, after, phases


async def start():
    return await oracle.OracleVerifier.start(["sender", "receiver"])


async def pending(verifier, attempts):
    with pytest.raises(oracle.OracleProofIncomplete, match="requires the final scenario daily close") as exc:
        await verifier.verify(attempts)
    return exc.value.diagnostics


@pytest.mark.parametrize("day", [DAY, NEXT])
async def test_open_day_does_not_count_as_a_completed_daily_close(snapshots, day):
    _, after, phases = snapshots
    after.update(post_snapshot(day=day))
    verifier = await start()

    diagnostics = await pending(verifier, [ATTEMPT])

    closed = [] if day == DAY else [DAY.isoformat()]
    assert diagnostics["scenario_business_dates"] == list(dict.fromkeys([DAY.isoformat(), day.isoformat()]))
    assert diagnostics["completed_daily_close_dates"] == closed
    assert diagnostics["verified_daily_close_dates"] == closed
    assert diagnostics["pending_daily_close"] == [day.isoformat()]
    assert diagnostics["open_ledger_coverage"] == [
        {"phase": "baseline", "business_date": DAY.isoformat(), "accounts_checked": 2, "mismatches": 0},
        {"phase": "post_scenario", "business_date": day.isoformat(), "accounts_checked": 2, "mismatches": 0},
    ]
    expected_observed = ZERO if closed else {k: v for k, v in ZERO.items() if k != "daily_close_mismatches"}
    assert diagnostics["observed_counters"] == expected_observed
    assert phases == ["baseline", "post-scenario"]


async def test_immediate_proof_can_complete_when_last_attempt_date_is_already_closed(snapshots):
    _, after, _ = snapshots
    after[oracle._CLOCK] = clock(NEXT)
    after[oracle._CLOSES] = [(1, DAY, 0, 900, "Y"), (2, DAY, 0, 300, "Y")]
    verifier = await start()

    proof = await verifier.verify([ATTEMPT])

    assert {k: proof[k] for k in ZERO} == ZERO
    assert proof["pending_daily_close"] == []
    assert proof["verified_daily_close_dates"] == [DAY.isoformat()]
    assert len(proof["open_ledger_coverage"]) == 1


async def test_eventual_proof_waits_for_a_natural_close_and_keeps_scenario_cutoff(snapshots):
    _, after, phases = snapshots
    verifier = await start()
    initial = await pending(verifier, [ATTEMPT])

    with pytest.raises(oracle.OracleProofIncomplete, match="requires the final scenario daily close") as exc:
        await verifier.verify_daily_close()
    assert exc.value.diagnostics == initial

    later = postings(sequence=3, transfer=23, request_id="later")
    after[oracle._CLOCK] = clock(NEXT + timedelta(days=1))
    after[oracle._LEDGER] = sorted(after[oracle._LEDGER] + later, key=lambda row: (row[0], row[1], row[2]))
    after[oracle._CLOSES] = [(1, DAY, 0, 800, "Y"), (2, DAY, 0, 400, "Y")]
    del after[oracle._REQUESTS]
    del after[oracle._MOVEMENTS]
    del after[oracle._UNLINKED]
    proof = await verifier.verify_daily_close()

    assert {k: proof[k] for k in ZERO} == ZERO
    assert proof["scenario_business_dates"] == [DAY.isoformat()]
    assert proof["verified_daily_close_dates"] == [DAY.isoformat()]
    assert proof["pending_daily_close"] == []
    assert proof["open_ledger_coverage"] == initial["open_ledger_coverage"]
    assert phases == ["baseline", "post-scenario", "daily-close", "daily-close"]


async def test_eventual_proof_cannot_complete_without_close_job_record(snapshots):
    _, after, _ = snapshots
    verifier = await start()
    await pending(verifier, [ATTEMPT])
    after[oracle._CLOCK] = clock(NEXT)
    after[oracle._FINISHED] = []

    with pytest.raises(oracle.OracleProofIncomplete, match="daily close job evidence is missing"):
        await verifier.verify_daily_close()


async def test_rejected_attempt_has_no_required_shape(snapshots):
    _, after, _ = snapshots
    after.update(
        {
            oracle._SCOPE: START,
            oracle._LEDGER: SEED,
            oracle._REQUESTS: [request(status="REJECTED")],
            oracle._MOVEMENTS: [],
        }
    )
    verifier = await start()
    rejection = dict(ATTEMPT, status=422, succeeded=False, business_rejection="405")

    assert (await pending(verifier, [rejection]))["observed_counters"] == {
        "duplicate_movements": 0,
        "f21_violations": 0,
    }


async def test_transport_loss_resolved_by_committed_outcome(snapshots):
    verifier = await start()

    diag = await pending(verifier, [dict(ATTEMPT, status=0, succeeded=False)])
    assert diag["observed_counters"] == {"duplicate_movements": 0, "f21_violations": 0}


async def test_replay_with_distinct_request_id_uses_same_committed_movement(snapshots):
    verifier = await start()

    diag = await pending(verifier, [ATTEMPT, dict(ATTEMPT, request_id="replay")])
    assert diag["observed_counters"] == {"duplicate_movements": 0, "f21_violations": 0}


@pytest.mark.parametrize("transport_loss", [False, True])
async def test_missing_request_is_incomplete(snapshots, transport_loss):
    _, after, _ = snapshots
    after[oracle._REQUESTS] = []
    verifier = await start()
    attempt = dict(ATTEMPT, status=0, succeeded=False) if transport_loss else ATTEMPT

    with pytest.raises(ValueError, match="outcome is missing"):
        await verifier.verify([attempt])


@pytest.mark.parametrize(
    ("query", "replacement", "message"),
    [
        (oracle._CLOCK, clock(NEXT, pending=1), "rollover is incomplete"),
        (oracle._SCOPE, START[:1], "account evidence is missing"),
        (oracle._FINISHED, [], "daily close job evidence is missing"),
        (oracle._CLOSES, [(1, DAY, 0, 1000, "Y")], "daily close account evidence is missing"),
        (oracle._REQUESTS, [request(fresh=0)], "evidence predates"),
    ],
)
async def test_missing_database_evidence_is_incomplete(snapshots, query, replacement, message):
    _, after, _ = snapshots
    after.update(post_snapshot(day=NEXT))
    after[query] = replacement
    verifier = await start()

    with pytest.raises(ValueError, match=message):
        await verifier.verify([ATTEMPT])


async def test_nonzero_baseline_mismatch_duplicate_and_bad_shape(snapshots):
    baseline, after, _ = snapshots
    baseline[oracle._SCOPE] = [account(START[0], 1001, 2), START[1]]
    duplicate = postings(sequence=3, transfer=21, amount=80)
    rows = postings() + duplicate
    after.update(
        {
            oracle._SCOPE: [account(START[0], 820, 4), account(START[1], 380, 4)],
            oracle._LEDGER: sorted(SEED + rows, key=lambda row: (row[0], row[2])),
            oracle._MOVEMENTS: movement(postings()) + movement(duplicate, amount=80),
        }
    )
    verifier = await start()

    diagnostics = await pending(verifier, [ATTEMPT])
    assert diagnostics["open_ledger_coverage"][0]["mismatches"] == 1
    assert diagnostics["open_ledger_coverage"][1]["mismatches"] == 0
    assert diagnostics["observed_counters"] == {"duplicate_movements": 1, "f21_violations": 1}

    after[oracle._CLOCK] = clock(NEXT)
    after[oracle._CLOSES] = [(1, DAY, 0, 820, "N"), (2, DAY, 0, 380, "Y")]
    proof = await verifier.verify_daily_close()
    assert {k: proof[k] for k in ZERO} == {
        "daily_close_mismatches": 1,
        "duplicate_movements": 1,
        "f21_violations": 1,
    }


async def test_reject_with_money_is_a_shape_violation(snapshots):
    _, after, _ = snapshots
    after[oracle._REQUESTS] = [request(status="REJECTED")]
    verifier = await start()

    diagnostics = await pending(
        verifier, [dict(ATTEMPT, status=422, succeeded=False, business_rejection=True)]
    )
    assert diagnostics["observed_counters"] == {"duplicate_movements": 0, "f21_violations": 1}


async def test_successful_request_with_no_movement_is_a_shape_violation(snapshots):
    _, after, _ = snapshots
    after.update({oracle._SCOPE: START, oracle._LEDGER: SEED, oracle._MOVEMENTS: []})
    verifier = await start()

    diag = await pending(verifier, [ATTEMPT])
    assert diag["observed_counters"] == {"duplicate_movements": 0, "f21_violations": 1}


async def test_crossed_close_is_independently_reconciled_even_if_marked_verified(snapshots):
    _, after, _ = snapshots
    after.update(post_snapshot(day=NEXT))
    after[oracle._CLOSES][0] = (1, DAY, 0, 999, "Y")
    verifier = await start()

    diag = await pending(verifier, [ATTEMPT])
    assert diag["observed_counters"] == {**ZERO, "daily_close_mismatches": 1}
    assert diag["completed_daily_close_dates"] == [DAY.isoformat()]
    assert diag["verified_daily_close_dates"] == []


async def test_driver_error_does_not_disclose_connection_details(monkeypatch):
    for name in oracle._ENV:
        monkeypatch.setenv(name, "credential-secret")

    def connect_async(**kwargs):
        raise oracle.oracledb.DatabaseError("credential-secret")

    monkeypatch.setattr(oracle.oracledb, "connect_async", connect_async)

    with pytest.raises(RuntimeError, match="Oracle N-5 baseline read failed") as result:
        await start()
    assert "credential-secret" not in str(result.value)
    assert result.value.__suppress_context__


async def test_bulk_documents_are_bound_as_temporary_clobs():
    temporary_lob = object()

    class Cursor:
        @property
        def connection(self):
            return self

        async def createlob(self, lob_type, value):
            assert lob_type is oracle.oracledb.DB_TYPE_CLOB
            assert value == '["sender","receiver"]'
            return temporary_lob

        async def execute(self, sql, binds):
            assert sql == oracle._LEDGER
            assert binds == {"accounts": temporary_lob, "first_day": DAY, "last_day": NEXT}

        async def fetchall(self):
            return SEED

    assert (
        await oracle._rows(
            Cursor(),
            oracle._LEDGER,
            documents=("accounts",),
            accounts=["sender", "receiver"],
            first_day=DAY,
            last_day=NEXT,
        )
        == SEED
    )


async def test_snapshot_starts_read_only_before_select(monkeypatch):
    commands = []

    class Cursor:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            pass

        async def execute(self, sql, *args):
            commands.append(sql)

        async def fetchall(self):
            return clock(DAY)

    class Connection:
        def cursor(self):
            return Cursor()

    @asynccontextmanager
    async def connect(**kwargs):
        yield Connection()

    monkeypatch.setattr(oracle.oracledb, "connect_async", connect)
    async with oracle._snapshot(("user", "password", "dsn"), "baseline") as cursor:
        assert await oracle._clock(cursor) == (DAY, STAMP)

    assert commands == [
        "ALTER SESSION SET CURRENT_SCHEMA = SALTATION",
        "SET TRANSACTION READ ONLY",
        oracle._CLOCK,
    ]


async def test_repeated_postings_under_one_transfer_are_duplicate_movements(snapshots):
    _, after, _ = snapshots
    rows = postings() + postings(sequence=3)
    after.update(
        {
            oracle._SCOPE: [account(START[0], 800, 4), account(START[1], 400, 4)],
            oracle._LEDGER: sorted(SEED + rows, key=lambda row: (row[0], row[2])),
            oracle._MOVEMENTS: movement(rows),
        }
    )
    verifier = await start()

    diag = await pending(verifier, [ATTEMPT])
    assert diag["observed_counters"] == {"duplicate_movements": 1, "f21_violations": 1}


async def test_unlinked_posting_on_an_out_of_scope_account_violates_shape(snapshots):
    _, after, _ = snapshots
    after[oracle._UNLINKED] = [(999, 1, 100, "request1")]
    verifier = await start()

    diagnostics = await pending(verifier, [ATTEMPT])

    assert diagnostics["observed_counters"] == {"duplicate_movements": 0, "f21_violations": 1}


async def test_verified_close_does_not_erase_open_ledger_mismatch(snapshots):
    baseline, after, _ = snapshots
    baseline[oracle._SCOPE] = [account(START[0], 1001, 2), START[1]]
    verifier = await start()
    await pending(verifier, [ATTEMPT])
    after[oracle._CLOCK] = clock(NEXT)
    after[oracle._CLOSES] = [(1, DAY, 0, 900, "Y"), (2, DAY, 0, 300, "Y")]

    with pytest.raises(oracle.OracleProofIncomplete, match="open ledger reconciliation found") as exc:
        await verifier.verify_daily_close()

    assert exc.value.diagnostics["verified_daily_close_dates"] == [DAY.isoformat()]
    assert exc.value.diagnostics["pending_daily_close"] == []
    assert exc.value.diagnostics["observed_counters"] == ZERO
    assert exc.value.diagnostics["open_ledger_coverage"][0]["mismatches"] == 1
