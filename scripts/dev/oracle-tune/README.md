# Populated-schema Oracle measurement

Run `scripts/dev/it-oracle.sh tune-seed` on the capture host. It recreates
only `SALTATION_TUNE`, using the existing test password and the reset script's
`SALTATION_DATA`/`OUTBOX_TS` quotas and application grants. The four existing
V$ grants are retained. Capture runs as SYSDBA, so no extra diagnostic grants
are required on the application user. SQL*Plus input uses AL32UTF8.

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
All accounts start at 100 million won; paired 100-won movements preserve funds.
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

Timed windows use normal session statistics with no logon trigger. After each
entry a separate short process runs with a temporary schema logon trigger setting
`STATISTICS_LEVEL=ALL`, including replacement pool connections. Its request warm-up
and window are one second each (outbox drains the complete fixture). Plan-run JSON
is kept separately as `plan-<entry>/plan.jsonl`; `bench.jsonl` contains timed results
only. An exit trap removes the trigger, including after failure.

At capture start, schema optimizer statistics are gathered once. Before every timed window
and plan entry the outbox is delivered to MongoDB, checked empty, and its tune
schema segment is truncated. `resets.txt` records each reset; drain logs record
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
