# Populated-schema Oracle measurement

Run `scripts/dev/it-oracle.sh tune-seed` on the capture host. It recreates
only `SALTATION_TUNE`, using the existing test password and the reset script's
`SALTATION_DATA`/`OUTBOX_TS` quotas and application grants. The four existing
V$ grants are retained. Capture runs as SYSDBA, so no extra diagnostic grants
are required on the application user. SQL*Plus input uses AL32UTF8.

For comparisons that cross a calendar date, set `TUNE_SEED_AT` to the same
RFC 3339 business timestamp on every new seed, for example
`TUNE_SEED_AT=2026-11-17T08:00:00+09:00 scripts/dev/it-oracle.sh tune-seed`.
It sets the freshly initialized, still-empty schema's business clock through
`Clock::set` before inserting customers. Without it the existing clock
initialization is unchanged. Check the persisted seed dates before comparing.

Defaults: `TUNE_CUSTOMERS=20000 TUNE_YEARS=3 TUNE_MAX_GB=25`. Start with
`TUNE_CUSTOMERS=500 TUNE_YEARS=1`. Before dropping the tune user, the script checks
free space on `/opt/oracle/oradata` inside the Oracle container (30 GiB minimum)
and caps the PDB's undo datafiles at 4 GiB, skipping files already larger than that.
Before each business-date batch it checks allocated segments in `SALTATION_DATA`, reserving
8 KiB per projected movement/account for the next batch. The limit includes
other users' segments in that tablespace; it is deliberately conservative.
The maximum accepted limit is 25 GiB. An aborted seed must be recreated.

The ignored `driver_bench::tune_prepare` test calls the real `Db::migrate` and
initializes the bank clock. No migration SQL is copied or parsed by the tooling.
The SQL seed uses `INSERT /*+ APPEND */ ... SELECT` row generators, with commits
between tables and dates to bound undo. Oracle may downgrade direct-path inserts
where enabled referential constraints require conventional inserts. Constraints
stay enabled and validated throughout. Explicit customer/account/transfer IDs
are followed by identity `START WITH LIMIT VALUE` so API inserts remain safe.

Each customer has a demand account, synthetic credential/PIN, fintech number,
opening inbound movement and paired payee. Reference products come from migrations.
Counterparties use the synthetic 9700 prefix; bank accounts use 9600. About 1% of
accounts have at least 50,000 transfer legs; most have one leg every other day
(about 550 in three years). Default volume is about 21 million ledger rows.
Demand accounts start at 100 million won; paired 100-won movements preserve funds.
The current business date comes from the migrated bank clock; the history starts
that date minus `TUNE_YEARS` calendar years. Daily closes cover completed dates,
while current-date daily totals remain available for posting. Posting reads
`balance`, `next_sequence`, `status`, `subtype` and `account_no`; the seed computes
running ledger balances, advances the next sequence and maintains daily totals.
Opening movements have one inbound leg; each transfer has two matching legs and
a completed request. Historical outbox rows are inserted and deleted in delivery
order, with at most 100 rows left as backlog. The seed does not deliver historical
events to MongoDB. Two shared Argon2 hashes are generated through the bank's
hashing code solely for the synthetic benchmark password and PIN.

After the daily loop, the seed adds one term deposit for each of
`FLOOR(TUNE_CUSTOMERS/10)` customers, starting with customers 1, 11, 21, and so on.
Maturities are evenly spaced from the first seeded date plus 30 days through the
business date plus 365 days. Matured deposits have CLOSED accounts and MATURITY
closures; future deposits remain ACTIVE. These synthetic product fixtures have
zero balances, next sequence 1, no ledger rows, and zero-balance daily closes for
all completed dates, preserving every consistency check. Each has a linked demand
account, fintech number, and status history.

The seed also adds `FLOOR(TUNE_CUSTOMERS/50)` consumed password resets spread over
the seeded period, using a fixed synthetic Argon2id PHC hash that is never logged,
and `FLOOR(TUNE_CUSTOMERS/100)` ended ATM sessions with a synthetic terminal.
ATM bearer token hashes use the feature's SHA-256 format. Reset expiry is five
minutes; ATM expiry is three minutes. Each of the first
`LEAST(TUNE_CUSTOMERS,1000)` customers gets up to three additional distinct payees,
excluding themselves and their existing paired payee; small populations receive
`LEAST(3,TUNE_CUSTOMERS-2)` extra payees. The post-consistency table counts include
DEPOSIT_DETAIL, PASSWORD_RESET, ATM_SESSION, and PAYEE.

`BENCH_POPULATED=1 ... driver-bench` skips schema reset and API population,
selects up to 1,000 seeded accounts and logs in through the public router.
Balance/history/hot-transfer selection prioritizes deep accounts; other workloads use a
repeatable hashed account order. Recipient pairs remain random and distinct.
Runs mutate the persistent population, so reseed when an identical starting
state is needed. Each populated JSONL line includes the persisted seed parameters;
normal benchmark JSON and fixture behavior are preserved.

`tune-capture` runs transfer:8, transfer:32, transfer:64, transfer_hot:32,
transfer_hot:64, history:8, balance:8, lookup:8 and outbox:1. `BENCH_RUNS`
overrides the timed run count. `transfer` retains its uniform selection;
`transfer_hot` uses the two deepest DEMAND accounts, distinct recipients, and
one-won amounts so repeated contention windows do not exhaust daily limits.
The populated outbox fixture contains 50,000 pending events and drives the real
production relay loop, including its backlog query before every batch.

Populated capture additionally runs all eight
`history_<window>_<page>_<depth>:8` combinations: windows `recent` and `old`,
pages `first` and `deep`, and account depths `shallow` and `deep`. Recent uses the
existing three-month period ending on the business date; old starts one calendar
month after TUNE_SEED.first_day and spans three months. Deep accounts have at
least 50,000 ledger rows; shallow accounts are ordinary seeded demand accounts.
For deep pages, untimed setup follows reusable sequence traces at most ten times,
retains the request for the reached page, and excludes single-page windows.
Timed requests repeat that saved page, including the last page when fewer than
eleven exist. No additional ledger history is created. `history_filtered_deep:8`
uses recent first pages on deep accounts with `inquiry_type=I`. The original
`history:8` request is unchanged.

`outbox_long:1` first queues `TUNE_OUTBOX_LONG` events (default 500000) with no
relay running, then drains them through the production relay and confirms Mongo
delivery during untimed setup. It retains the outbox segment after that drain.
The measured window queues and delivers 1000 more events through the production
relay, reporting 1000 successes. The backlog cursor is parsed before the start
snapshot. For a smaller verification fixture, pass `TUNE_OUTBOX_LONG=20000` to
`tune-capture`; the value is forwarded to both timed and separate plan processes.
The nine new history cases and long outbox case require `BENCH_POPULATED=1`.

Timed windows use normal session statistics with no logon trigger. After each
entry a separate short process runs with a temporary schema logon trigger setting
`STATISTICS_LEVEL=ALL`, including replacement pool connections. Its request warm-up
and window are one second each (outbox drains the complete fixture). Plan-run JSON
is kept separately as `plan-<entry>/plan.jsonl`; `bench.jsonl` contains timed results
only. An exit trap removes the trigger, including after failure.

At capture start, schema optimizer statistics are gathered once. Before every timed window
and plan entry the outbox is delivered to MongoDB, checked empty, and its tune
schema segment is truncated. No truncate occurs between the long outbox setup
and its measured window. `resets.txt` records each reset; drain logs record
failures. Other persistent data is not reset: reseed for identical starting data.

The snapshot request/ack protocol records database-time bounds in `TUNE_WINDOWS`.
Each start/end snapshot includes V$SQL child counters and the first 60 characters
of SQL text. `sqldelta.py START END` subtracts counters by SQL ID and child into
`sqldelta-<entry>-run<n>.txt`. New, reset, reloaded and missing/aged-out cursors are marked;
missing, reloaded or reset values are unknown rather than invented deltas. These snapshots
are not atomic across cursors, and cursor eviction/reload can limit attribution.
ASH exports cover timed uniform and hot transfer windows, grouped by run. Empty
ASH profiles are possible; samples are not wait durations. No AWR snapshot or
system-level setting is changed.

XPLAN matches exact production SQL-file text, ignoring terminal whitespace, and
uses `ALLSTATS LAST +PEEKED_BINDS`. Only cursors active since the separate plan
run began are included in `xplan-<entry>.txt`. Setup and warm-up in that plan
process may also execute production statements. The final ignored `tune_coverage`
test inserts minimal fixtures and executes additional production SQL through the
real adapter in one rolled-back transaction. `coverage.txt` records the execution
manifest; `xplan-coverage.txt` enumerates production files with captured cursors or
`not executed: <reason>` markers for unavailable cursors.

The capture host (`uname -a`), benchmark container limits and Oracle/Mongo container
limits are recorded in `host.txt` in the evidence directory. Segment/index summaries
are recorded after the run. Evidence lives under
`.evidence/oracle-tuning/<UTC timestamp>/`; measured performance stays there and is
never written into this README or other tracked files.

Every result line includes `oracle_cpus` and `oracle_memory` strings from the
Oracle container's Docker limits. CPUs are decimal CPU units and memory is in
bytes; a zero Docker limit is reported as `unlimited`.

`counters.txt` contains session counter deltas per successful request and the
absolute logon delta for each captured entry and run, followed by per-cursor
executions per successful request and buffer gets per execution. Session counters
match SID and serial number.
If the session identity/statistic sets change or a counter decreases, session
deltas are unknown (`?`): summing only end survivors loses the work of retired
sessions and can make the old pool appear artificially cheaper. Stable endpoint
identities alone cannot exclude connections opened and closed between snapshots.
The report also shows database-lifetime counter deltas already present in the
raw snapshots. These retain retired-session work in the snapshot container,
but may include diagnostic and background activity. Compare them only across the same isolated capture protocol;
do not attribute their raw totals or logon counts exclusively to the tune schema.
The snapshot observer and window-bound writer can contribute round trips and
commits; do not subtract an assumed observer logon count (SYS authenticates in
the root before switching to the PDB). Diagnostic work is not silently treated
as application work.
Zero denominators produce `n/a`; unknown cursor deltas produce `?`. Counter
regression tests run before every capture.

Outbox drain and truncate happen before the benchmark process and its warm-up.
After fixture population, the benchmark warms the backlog query before the start
snapshot, so a cursor invalidated by truncate is reparsed outside the timed window.
No truncate or statistics gathering runs between the start and end snapshots.
The final counters summary runs after segment capture; summary failures fail capture.
