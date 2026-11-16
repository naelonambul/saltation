# Populated-schema Oracle measurement

Run `scripts/dev/it-oracle.sh tune-seed` on the OCI Docker host. It recreates
only `SALTATION_TUNE`, using the existing test password and the reset script's
`SALTATION_DATA`/`OUTBOX_TS` quotas and application grants. The four existing
V$ grants are retained. Capture runs as SYSDBA, so no extra diagnostic grants
are required on the application user. SQL*Plus input uses AL32UTF8.

Defaults: `TUNE_CUSTOMERS=20000 TUNE_YEARS=3 TUNE_MAX_GB=25`. Start with
`TUNE_CUSTOMERS=500 TUNE_YEARS=1`. The script checks free space on the Docker
root filesystem before dropping the tune user (30 GiB minimum). Before each
business-date batch it checks allocated segments in `SALTATION_DATA`, reserving
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
Balance/history selection prioritizes deep accounts; other workloads use a
repeatable hashed account order. Recipient pairs remain random and distinct.
Runs mutate the persistent population, so reseed when an identical starting
state is needed. Each populated JSONL line includes the persisted seed parameters;
normal benchmark JSON and fixture behavior are preserved.

`tune-capture` defaults to one run of transfer:8, transfer:32, transfer:64,
history:8, balance:8, lookup:8 and outbox:1. `BENCH_RUNS` overrides the run count.
A temporary schema logon trigger sets session-level `STATISTICS_LEVEL=ALL` for
all tune pool connections, including replacements. An exit trap removes it.
The existing snapshot request/ack protocol records database-time window bounds
in `TUNE_WINDOWS`; ASH exports include only tune-user samples within each transfer
window, grouped by run. Empty ASH results are possible, and sample counts are
not wait durations. No AWR snapshot or system-level setting is changed.

XPLAN captures each cached child with executions for an exact production SQL-file
text match, ignoring terminal whitespace, using `ALLSTATS LAST +PEEKED_BINDS`.
Reports enumerate all production SQL files, a superset of findings 1–8. They are
cumulative cached-cursor snapshots taken after each workload, including setup and
warm-up; they are not per-window execution deltas. Statements not executed by
these workloads (for example ATM or maturity jobs), or already aged out of the
shared pool, are explicitly marked `NOT EXECUTED / NOT IN SHARED POOL`. These
markers are gaps to address in the later baseline measurement, not invented plans.
Statistics and segment/index leaf-block summaries are recorded after the run.
All evidence lives under `.evidence/oracle-tuning/<UTC timestamp>/`; the baseline
document remains a skeleton until the coordinator performs the full-size run.
