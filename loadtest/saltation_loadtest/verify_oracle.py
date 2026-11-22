"""Read-only Oracle evidence for N-5 on isolated in-bank load-test accounts."""

from __future__ import annotations

import json
import os
import re
from collections import Counter, defaultdict
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import date, timedelta
from typing import NamedTuple

import oracledb

_DAY = timedelta(days=1)
_ENV = ("SALTATION_DB_USER", "SALTATION_DB_PASSWORD", "SALTATION_DB_CONNECT")


def oracle_schema() -> str:
    schema = os.environ.get("SALTATION_VERIFY_ORACLE_SCHEMA", "saltation").upper()
    if not re.fullmatch(r"[A-Z][A-Z0-9_]{0,29}", schema):
        raise ValueError("invalid Oracle proof schema")
    return schema


_CLOCK = """
SELECT TRUNC(CAST((CASE WHEN clock_mode = 'LIVE' THEN SYSTIMESTAMP
                       ELSE business_time END) AT TIME ZONE '+09:00' AS TIMESTAMP)),
       rolled_through,
       (SELECT COUNT(*) FROM rollover_run r WHERE r.business_date > sim_clock.rolled_through),
       TO_CHAR(SYS_EXTRACT_UTC(SYSTIMESTAMP), 'YYYY-MM-DD HH24:MI:SS.FF6')
FROM sim_clock WHERE id = 'X'
"""

_SCOPE = """
SELECT a.account_id, a.account_no, a.customer_id, a.balance, a.next_sequence,
       a.status, TRUNC(CAST(a.opened_at AT TIME ZONE '+09:00' AS TIMESTAMP)),
       NVL(p.account_sequence, 0), NVL(p.balance_after, 0),
       c.closing_balance, c.verified,
       CASE WHEN EXISTS (SELECT 1 FROM daily_close d WHERE d.business_date < :first_day)
            THEN 1 ELSE 0 END
FROM JSON_TABLE(:accounts, '$[*]' COLUMNS (account_no VARCHAR2(16) PATH '$')) j
JOIN account a ON a.account_no = j.account_no
OUTER APPLY (
    SELECT r.account_sequence, r.balance_after
    FROM account_transaction r
    WHERE r.account_id = a.account_id AND r.business_date < :first_day
    ORDER BY r.business_date DESC, r.account_sequence DESC
    FETCH FIRST 1 ROW ONLY
) p
LEFT JOIN daily_close c ON c.account_id = a.account_id AND c.business_date = :first_day - 1
"""

_LEDGER = """
SELECT r.account_id, r.business_date, r.account_sequence, r.amount, r.balance_after,
       r.request_id, r.transfer_id, r.channel
FROM JSON_TABLE(:accounts, '$[*]' COLUMNS (account_id NUMBER PATH '$')) j
JOIN account_transaction r ON r.account_id = j.account_id
WHERE r.business_date BETWEEN :first_day AND :last_day
ORDER BY r.account_id, r.business_date, r.account_sequence
"""

_FINISHED = """
SELECT business_date FROM rollover_run
WHERE job = 'daily_close' AND finished_at IS NOT NULL
  AND business_date >= :first_day AND business_date < :last_day
"""

_CLOSES = """
SELECT c.account_id, c.business_date, c.opening_balance, c.closing_balance, c.verified
FROM daily_close c
WHERE c.business_date >= :first_day AND c.business_date < :last_day
  AND (c.verified = 'N' OR c.account_id IN (
      SELECT j.account_id
      FROM JSON_TABLE(:accounts, '$[*]' COLUMNS (account_id NUMBER PATH '$')) j
  ))
"""

_REQUESTS = """
WITH sent AS (
    SELECT j.request_id, j.bank_tran_id, j.idem_scope
    FROM JSON_TABLE(:attempts, '$[*]' COLUMNS (
        request_id VARCHAR2(26) PATH '$[0]',
        bank_tran_id VARCHAR2(20) PATH '$[1]',
        idem_scope VARCHAR2(40) PATH '$[2]'
    )) j
), candidates AS (
    SELECT r.request_id FROM sent s JOIN transaction_request r ON r.request_id = s.request_id
    UNION
    SELECT r.request_id FROM sent s JOIN transaction_request r
      ON r.idem_scope = s.idem_scope AND r.idem_key = s.bank_tran_id
)
SELECT r.request_id, r.bank_tran_id, r.idem_scope, r.idem_key, r.request_type, r.status,
       TRUNC(CAST(r.occurred_at AT TIME ZONE '+09:00' AS TIMESTAMP)),
       CASE WHEN SYS_EXTRACT_UTC(r.recorded_at) >=
           TO_TIMESTAMP(:started_at, 'YYYY-MM-DD HH24:MI:SS.FF6') THEN 1 ELSE 0 END
FROM candidates c JOIN transaction_request r ON r.request_id = c.request_id
"""

_MOVEMENTS = """
WITH candidates AS (
    SELECT t.transfer_id
    FROM JSON_TABLE(:requests, '$[*]' COLUMNS (request_id VARCHAR2(26) PATH '$')) j
    JOIN transfer t ON t.request_id = j.request_id
    UNION
    SELECT j.transfer_id
    FROM JSON_TABLE(:transfers, '$[*]' COLUMNS (transfer_id NUMBER PATH '$')) j
)
SELECT t.transfer_id, t.request_id, t.bank_tran_id, t.kind, t.amount,
       r.account_id, r.account_sequence, r.amount, r.request_id, r.business_date
FROM candidates c JOIN transfer t ON t.transfer_id = c.transfer_id
LEFT JOIN account_transaction r ON r.transfer_id = t.transfer_id
"""


_UNLINKED = """
SELECT r.account_id, r.account_sequence, r.amount, r.request_id
FROM JSON_TABLE(:requests, '$[*]' COLUMNS (request_id VARCHAR2(26) PATH '$')) j
JOIN account_transaction r ON r.request_id = j.request_id
LEFT JOIN transfer t ON t.transfer_id = r.transfer_id
WHERE r.occurred_at >= FROM_TZ(CAST(:first_day AS TIMESTAMP), '+09:00')
  AND r.occurred_at < FROM_TZ(CAST(:until_day AS TIMESTAMP), '+09:00')
  AND (t.request_id IS NULL OR t.request_id <> r.request_id)
"""


class _Account(NamedTuple):
    id: int
    number: str
    customer: int
    balance: int
    next_sequence: int
    status: str
    opened: date
    prior_sequence: int
    prior_balance: int
    prior_close: int | None
    prior_verified: str | None
    has_close_history: int


class _Posting(NamedTuple):
    account: int
    day: date
    sequence: int
    amount: int
    balance: int
    request: str | None
    transfer: int | None
    channel: str


class _Request(NamedTuple):
    id: str
    bank_id: str | None
    scope: str | None
    key: str | None
    kind: str
    status: str
    day: date
    fresh: int


class _Movement(NamedTuple):
    id: int
    request: str | None
    bank_id: str | None
    kind: str | None
    amount: int


class _Leg(NamedTuple):
    account: int
    sequence: int
    amount: int
    request: str | None
    day: date


@dataclass(frozen=True)
class _Attempt:
    request: str
    bank_id: str
    sender: _Account
    receiver: _Account
    amount: int
    succeeded: bool
    business_rejection: bool

    @property
    def key(self) -> tuple[str, str]:
        return f"C:{self.sender.customer}", self.bank_id


def _date(value: date) -> date:
    # Oracle DATE comes back as a datetime at midnight.
    return date(value.year, value.month, value.day)


@asynccontextmanager
async def _snapshot(settings: tuple[str, str, str], phase: str):
    user, password, dsn = settings
    try:
        async with oracledb.connect_async(user=user, password=password, dsn=dsn) as connection:
            connection.call_timeout = 60_000
            with connection.cursor() as cursor:
                if user.upper() != oracle_schema():
                    await cursor.execute(f"ALTER SESSION SET CURRENT_SCHEMA = {oracle_schema()}")
                cursor.arraysize = 2000
                await cursor.execute("SET TRANSACTION READ ONLY")
                yield cursor
    except (oracledb.Error, OSError, ValueError) as exc:
        # The driver can include operator-supplied connection details in exception text.
        # Preserve our own validation errors, which have fixed messages.
        if isinstance(exc, OracleProofIncomplete):
            raise
        raise RuntimeError(f"Oracle N-5 {phase} read failed") from None


class OracleProofIncomplete(ValueError):
    """Evidence is incomplete. Diagnostics are safe to include in the load-test report."""

    def __init__(self, message: str, diagnostics: dict | None = None):
        super().__init__(message)
        self.diagnostics = {} if diagnostics is None else diagnostics


async def _rows(cursor, sql: str, *, documents: tuple[str, ...] = (), **binds) -> list[tuple]:
    # Explicit temporary CLOBs keep large attempt lists within a single bulk
    # query on Oracle 19c, without a table write or a VARCHAR2-sized bind.
    for name in documents:
        payload = json.dumps(binds[name], separators=(",", ":"))
        binds[name] = await cursor.connection.createlob(oracledb.DB_TYPE_CLOB, payload)
    await cursor.execute(sql, binds)
    return await cursor.fetchall()


async def _clock(cursor) -> tuple[date, str]:
    rows = await _rows(cursor, _CLOCK)
    if len(rows) != 1:
        raise OracleProofIncomplete("Oracle N-5 business clock evidence is missing")
    day, rolled, pending, timestamp = rows[0]
    if day is None or rolled is None or pending or _date(rolled) != _date(day) - _DAY:
        raise OracleProofIncomplete("Oracle N-5 business clock rollover is incomplete")
    return _date(day), timestamp


async def _accounts(
    cursor, names: list[str], first: date, *, require_open: bool = True
) -> dict[str, _Account]:
    rows = await _rows(cursor, _SCOPE, documents=("accounts",), accounts=names, first_day=first)
    accounts = {row[1]: _Account(*row) for row in rows}
    if accounts.keys() != set(names):
        raise OracleProofIncomplete("Oracle N-5 account evidence is missing")
    if any(_date(a.opened) > first or (require_open and a.status == "CLOSED") for a in accounts.values()):
        raise OracleProofIncomplete("Oracle N-5 accounts must be open throughout the scenario")
    return accounts


async def _postings(cursor, ids: list[int], first: date, last: date) -> list[_Posting]:
    rows = await _rows(cursor, _LEDGER, documents=("accounts",), accounts=ids, first_day=first, last_day=last)
    return [_Posting(*row) for row in rows]


def _reconcile(
    accounts: dict[str, _Account],
    postings: list[_Posting],
    first: date,
    last: date,
    closes: list[tuple],
    *,
    open_day: date | None,
) -> set[tuple[int, date]]:
    stored = {
        (account, _date(day)): (opening, closing, verified)
        for account, day, opening, closing, verified in closes
    }
    mismatches = {key for key, (_, _, verified) in stored.items() if verified != "Y"}
    by_day = defaultdict(list)
    for posting in postings:
        by_day[posting.account, _date(posting.day)].append(posting)

    for account in accounts.values():
        balance = account.prior_balance
        sequence = account.prior_sequence + 1
        day = first
        if account.prior_close is not None:
            if account.prior_close != balance or account.prior_verified != "Y":
                mismatches.add((account.id, first))
        elif account.has_close_history and _date(account.opened) < first:
            raise OracleProofIncomplete("Oracle N-5 previous daily close evidence is missing")

        while day <= last:
            key = account.id, day
            opening = balance
            for posting in by_day[key]:
                balance += posting.amount
                if (
                    posting.balance != balance
                    or posting.sequence != sequence
                    or posting.transfer is None
                    or (posting.request is None and posting.channel != "SYSTEM")
                ):
                    mismatches.add(key)
                sequence += 1
            if day != open_day:
                if key not in stored:
                    raise OracleProofIncomplete("Oracle N-5 daily close account evidence is missing")
                if stored[key][:2] != (opening, balance):
                    mismatches.add(key)
            elif (balance, sequence) != (account.balance, account.next_sequence):
                mismatches.add(key)
            day += _DAY
    return mismatches


def _attempts(records: list[dict], accounts: dict[str, _Account]) -> list[_Attempt]:
    if not records:
        raise OracleProofIncomplete("Oracle N-5 attempt evidence is empty")
    attempts = []
    by_request = {}
    by_key = {}
    for record in records:
        try:
            request = record["request_id"]
            bank_id = record["bank_tran_id"]
            amount = record["amount"]
            status = record["status"]
            succeeded = record["succeeded"]
            rejected = record["business_rejection"]
            sender = accounts[record["sender"]]
            receiver = accounts[record["receiver"]]
        except (KeyError, TypeError):
            raise OracleProofIncomplete("Oracle N-5 attempt fields or account scope are incomplete") from None
        if (
            not isinstance(request, str)
            or not 0 < len(request) <= 26
            or not isinstance(bank_id, str)
            or not 0 < len(bank_id) <= 20
            or type(amount) is not int
            or not 0 < amount <= 999_999_999_999
            or type(status) is not int
            or not (status == 0 or 100 <= status <= 599)
            or type(succeeded) is not bool
            or (rejected is not None and not isinstance(rejected, (bool, str)))
            or (succeeded and (status != 200 or rejected))
            or (rejected and status != 422)
            or sender == receiver
        ):
            raise OracleProofIncomplete("Oracle N-5 attempt fields are invalid")
        attempt = _Attempt(request, bank_id, sender, receiver, amount, succeeded, bool(rejected))
        shape = sender.id, receiver.id, amount
        if by_request.setdefault(request, attempt.key) != attempt.key or (
            by_key.setdefault(attempt.key, shape) != shape
        ):
            raise OracleProofIncomplete("Oracle N-5 attempt identities conflict")
        attempts.append(attempt)
    return attempts


def _check_movements(
    attempts: list[_Attempt],
    requests: dict[str, _Request],
    movements: dict[_Movement, list[_Leg]],
    unlinked_postings: list[tuple],
    accounts: dict[str, _Account],
    first: date,
    last: date,
) -> tuple[int, int]:
    by_key = {attempt.key: attempt for attempt in attempts}
    request_keys = {attempt.request: attempt.key for attempt in attempts}
    canonical = {}
    for request in requests.values():
        key = request.scope, request.key
        if key in by_key:
            canonical[key] = request
            request_keys.setdefault(request.id, key)

    bank_keys = defaultdict(set)
    for key in by_key:
        bank_keys[key[1]].add(key)

    baseline_sequences = {account.id: account.next_sequence for account in accounts.values()}
    transfer_counts = Counter()
    posting_roles = Counter()
    violations = set()
    violations_by_attempt = set()
    counted_legs = set()
    for movement, legs in movements.items():
        key = request_keys.get(movement.request)
        if key is None:
            candidates = bank_keys.get(movement.bank_id, set())
            if movement.kind != "transfer" and not candidates:
                continue
            if len(candidates) != 1:
                raise OracleProofIncomplete("Oracle N-5 could not attribute a scoped transfer")
            key = next(iter(candidates))
        expected = by_key[key]
        transfer_counts[key] += 1
        expected_legs = Counter(
            {(expected.sender.id, -expected.amount): 1, (expected.receiver.id, expected.amount): 1}
        )
        actual_legs = Counter((leg.account, leg.amount) for leg in legs)
        for leg in legs:
            counted_legs.add((key, leg.account, leg.sequence))
            posting_roles[key, leg.account, leg.amount > 0] += leg.amount != 0
            if leg.account in baseline_sequences and leg.sequence < baseline_sequences[leg.account]:
                raise OracleProofIncomplete("Oracle N-5 posting evidence predates the scenario")
        request = requests.get(movement.request)
        if (
            request is None
            or request.status != "DONE"
            or request.kind != "TRANSFER"
            or request.bank_id != expected.bank_id
            or (request.scope, request.key) != key
            or movement.kind != "transfer"
            or movement.amount != expected.amount
            or movement.bank_id != expected.bank_id
            or actual_legs != expected_legs
            or any(leg.request != movement.request or not first <= _date(leg.day) <= last for leg in legs)
        ):
            violations.add(movement.id)

    movement_requests = {movement.request for movement in movements}
    for account, sequence, amount, request_id in unlinked_postings:
        key = request_keys[request_id]
        if (key, account, sequence) not in counted_legs:
            posting_roles[key, account, amount > 0] += amount != 0
        violations_by_attempt.add(key)

    for attempt in attempts:
        exact = requests.get(attempt.request)
        request = exact or canonical.get(attempt.key)
        if request is None:
            # With a lost response the server can still be processing at the snapshot.
            raise OracleProofIncomplete("Oracle N-5 attempt outcome is missing or unresolved")
        if (
            request.kind != "TRANSFER"
            or request.bank_id != attempt.bank_id
            or request.status not in {"DONE", "REJECTED"}
            or (request.scope is not None and (request.scope, request.key) != attempt.key)
            or (attempt.succeeded and request.status != "DONE")
            or (attempt.business_rejection and request.status != "REJECTED")
        ):
            violations_by_attempt.add(attempt.key)
        if request.status == "DONE" and request.id not in movement_requests:
            violations_by_attempt.add(attempt.key)

    duplicates = {key for key, count in transfer_counts.items() if count > 1}
    duplicates.update(key for (key, _, _), count in posting_roles.items() if count > 1)
    return len(duplicates), len(violations) + len(violations_by_attempt)


@dataclass(frozen=True)
class _ScenarioEvidence:
    last: date
    open_day: date | None
    postings: list[_Posting]
    next_sequences: dict[int, int]
    mismatches: set[tuple[int, date]]
    duplicates: int
    f21: int


class OracleVerifier:
    """Read the scenario ledger immediately and confirm its close after normal rollover."""

    def __init__(
        self,
        settings: tuple[str, str, str],
        first: date,
        started_at: str,
        accounts: dict[str, _Account],
        postings: list[_Posting],
        mismatches: set[tuple[int, date]],
    ):
        self._settings = settings
        self._first = first
        self._started_at = started_at
        self._accounts = accounts
        self._postings = postings
        self._mismatches = mismatches
        self._scenario: _ScenarioEvidence | None = None

    @classmethod
    async def start(cls, accounts: list[str]) -> OracleVerifier:
        if not accounts or any(not isinstance(a, str) or not 0 < len(a) <= 16 for a in accounts):
            raise ValueError("Oracle N-5 requires valid account numbers")
        user, password, dsn = (os.environ.get(name, "") for name in _ENV)
        settings = user, password, dsn
        if not all(settings):
            raise ValueError(
                "Oracle N-5 requires SALTATION_DB_USER, SALTATION_DB_PASSWORD and SALTATION_DB_CONNECT"
            )
        if settings[0].upper() == "SYS":
            raise ValueError("Oracle N-5 requires a non-SYS database user")

        async with _snapshot(settings, "baseline") as cursor:
            first, started_at = await _clock(cursor)
            baseline = await _accounts(cursor, sorted(set(accounts)), first)
            ids = [a.id for a in baseline.values()]
            postings = await _postings(cursor, ids, first, first)
        mismatches = _reconcile(baseline, postings, first, first, [], open_day=first)
        return cls(settings, first, started_at, baseline, postings, mismatches)

    async def _read_ledger(
        self, cursor, last: date, *, open_day: date | None
    ) -> tuple[dict[str, _Account], list[_Posting], set[tuple[int, date]]]:
        first = self._first
        baseline = self._accounts
        accounts = await _accounts(cursor, list(baseline), first, require_open=open_day is not None)
        if any(
            (a.id, a.customer) != (baseline[name].id, baseline[name].customer)
            or a.next_sequence < baseline[name].next_sequence
            for name, a in accounts.items()
        ):
            raise OracleProofIncomplete("Oracle N-5 account identity or ledger sequence changed unexpectedly")

        ids = [a.id for a in baseline.values()]
        postings = await _postings(cursor, ids, first, last)
        sequences = {a.id: a.next_sequence for a in baseline.values()}
        retained = [p for p in postings if p.sequence < sequences[p.account]]
        if retained != self._postings:
            raise OracleProofIncomplete("Oracle N-5 baseline ledger evidence changed")

        closes = []
        closed_before = last if open_day is not None else last + _DAY
        if closed_before > first:
            bounds = {"first_day": first, "last_day": closed_before}
            finished = await _rows(cursor, _FINISHED, **bounds)
            if len({_date(day) for (day,) in finished}) != (closed_before - first).days:
                raise OracleProofIncomplete("Oracle N-5 daily close job evidence is missing")
            closes = await _rows(cursor, _CLOSES, documents=("accounts",), accounts=ids, **bounds)
        mismatches = _reconcile(accounts, postings, first, last, closes, open_day=open_day)
        return accounts, postings, mismatches

    def _coverage(
        self, scenario: _ScenarioEvidence, closed_mismatches: set[tuple[int, date]], *, complete: bool
    ) -> dict:
        first, last = self._first, scenario.last
        scenario_dates = [first + i * _DAY for i in range((last - first).days + 1)]
        closed_dates = scenario_dates if complete else scenario_dates[:-1]
        bad_dates = {day for _, day in closed_mismatches}
        open_ledgers = [
            {
                "phase": "baseline",
                "business_date": first.isoformat(),
                "accounts_checked": len(self._accounts),
                "mismatches": len(self._mismatches),
            }
        ]
        if scenario.open_day is not None:
            open_ledgers.append(
                {
                    "phase": "post_scenario",
                    "business_date": scenario.open_day.isoformat(),
                    "accounts_checked": len(self._accounts),
                    "mismatches": sum(day == scenario.open_day for _, day in scenario.mismatches),
                }
            )
        return {
            "scenario_business_dates": [day.isoformat() for day in scenario_dates],
            "open_ledger_coverage": open_ledgers,
            "completed_daily_close_dates": [day.isoformat() for day in closed_dates],
            "verified_daily_close_dates": [day.isoformat() for day in closed_dates if day not in bad_dates],
            "pending_daily_close": [] if complete else [last.isoformat()],
        }

    def _pending_close(self, scenario: _ScenarioEvidence) -> OracleProofIncomplete:
        closed_mismatches = {(account, day) for account, day in scenario.mismatches if day < scenario.last}
        diagnostics = self._coverage(scenario, closed_mismatches, complete=False)
        diagnostics["observed_counters"] = {
            "duplicate_movements": scenario.duplicates,
            "f21_violations": scenario.f21,
        }
        if diagnostics["completed_daily_close_dates"]:
            diagnostics["observed_counters"]["daily_close_mismatches"] = len(closed_mismatches)
        return OracleProofIncomplete("Oracle N-5 requires the final scenario daily close", diagnostics)

    async def verify(self, attempts: list[dict]) -> dict:
        """Read one post-scenario snapshot and report any pending daily close as incomplete."""
        if self._scenario is not None:
            raise self._pending_close(self._scenario)
        sent = _attempts(attempts, self._accounts)
        first = self._first
        async with _snapshot(self._settings, "post-scenario") as cursor:
            active, _ = await _clock(cursor)
            if active < first:
                raise OracleProofIncomplete("Oracle N-5 business date moved backwards")
            keys = sorted({(a.request, a.bank_id, a.key[0]) for a in sent})
            request_rows = await _rows(
                cursor, _REQUESTS, documents=("attempts",), attempts=keys, started_at=self._started_at
            )
            requests = {row[0]: _Request(*row) for row in request_rows}
            if not requests:
                raise OracleProofIncomplete("Oracle N-5 attempt outcome is missing or unresolved")
            request_dates = {_date(r.day) for r in requests.values()}
            if any(not r.fresh for r in requests.values()) or min(request_dates) < first:
                raise OracleProofIncomplete("Oracle N-5 request evidence predates the scenario")
            last = max(request_dates)
            if last > active:
                raise OracleProofIncomplete("Oracle N-5 request is outside the observed business dates")
            open_day = last if last == active else None
            accounts, postings, mismatches = await self._read_ledger(cursor, last, open_day=open_day)
            sequences = {a.id: a.next_sequence for a in self._accounts.values()}
            request_ids = sorted(requests.keys() | {a.request for a in sent})
            new_postings = [p for p in postings if p.sequence >= sequences[p.account]]
            rows = await _rows(
                cursor,
                _MOVEMENTS,
                documents=("requests", "transfers"),
                requests=request_ids,
                transfers=sorted({p.transfer for p in new_postings if p.transfer is not None}),
            )
            unlinked_postings = await _rows(
                cursor,
                _UNLINKED,
                documents=("requests",),
                requests=request_ids,
                first_day=first,
                until_day=last + _DAY,
            )
        movements = defaultdict(list)
        for row in rows:
            legs = movements[_Movement(*row[:5])]
            if row[5] is not None:
                legs.append(_Leg(*row[5:]))

        duplicate_movements, f21_violations = _check_movements(
            sent, requests, movements, unlinked_postings, self._accounts, first, last
        )
        scenario = _ScenarioEvidence(
            last,
            open_day,
            postings,
            {a.id: a.next_sequence for a in accounts.values()},
            mismatches,
            duplicate_movements,
            f21_violations,
        )
        if open_day is None:
            return self._complete(scenario, mismatches)
        self._scenario = scenario
        raise self._pending_close(scenario)

    async def verify_daily_close(self) -> dict:
        """Finish the captured proof after normal rollover has closed its last business date."""
        scenario = self._scenario
        if scenario is None:
            raise OracleProofIncomplete("Oracle N-5 requires post-scenario evidence before daily close")
        async with _snapshot(self._settings, "daily-close") as cursor:
            active, _ = await _clock(cursor)
            if active < scenario.last:
                raise OracleProofIncomplete("Oracle N-5 business date moved backwards")
            if active == scenario.last:
                raise self._pending_close(scenario)
            _, postings, closed_mismatches = await self._read_ledger(cursor, scenario.last, open_day=None)

        retained = [p for p in postings if p.sequence < scenario.next_sequences[p.account]]
        if retained != scenario.postings:
            raise OracleProofIncomplete("Oracle N-5 post-scenario ledger evidence changed before daily close")
        return self._complete(scenario, closed_mismatches)

    def _complete(self, scenario: _ScenarioEvidence, closed_mismatches: set[tuple[int, date]]) -> dict:
        open_mismatches = self._mismatches | {
            (account, day) for account, day in scenario.mismatches if day == scenario.open_day
        }
        coverage = self._coverage(scenario, closed_mismatches, complete=True)
        counters = {
            "daily_close_mismatches": len(closed_mismatches),
            "duplicate_movements": scenario.duplicates,
            "f21_violations": scenario.f21,
        }
        if open_mismatches - closed_mismatches:
            # A later close cannot establish that the earlier open ledger was correct.
            raise OracleProofIncomplete(
                "Oracle N-5 open ledger reconciliation found mismatches",
                {**coverage, "observed_counters": counters},
            )
        return {**counters, **coverage}
