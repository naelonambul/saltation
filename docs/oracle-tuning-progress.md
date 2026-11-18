# Oracle tuning implementation progress

This records implementation and verification for the approved
`changes/oracle-tuning/plan.md`. It does not replace the plan or its acceptance
criteria. Performance results remain in ignored `.evidence/` directories.

## Session overhead

The step-3 candidate tracks possible transactions before statements execute,
rolls back after errors or unfinished transactions, and retains sessions after
recoverable lock timeouts. Call timeouts, failed commits, failed rollbacks and
panics retire sessions. Oracle alternative-quoted SQL is conservatively treated
as potentially transactional so comments inside literals cannot hide a lock.

The coordinator reran format, Clippy, unit tests, `it-oracle.sh db-pool` and
`it-oracle.sh all` on OCI. Full repository verification passed with a complete
run at `.evidence/20261116T235852951052Z/evidence.json` in the step-3 worktree.
Its independent review found no remaining substantive issues. The fresh-seed
populated comparison completed every required workload and passed the step-3
criteria. Counter improvements also hold for every candidate run against the
reference range, rather than only at the median. The comparison and audit are
under `.evidence/oracle-tuning/handoff-20261117/step3-full-comparison.txt` and
`step3-every-run-counter-audit.json` in the step-3 worktree. The capture is
`.evidence/oracle-tuning/20261117T003854Z/` there.

The reference hot-account runs retired sessions during their timed windows.
The original report summed only end survivors and therefore omitted some work.
The corrected report marks changed/reset session samples as unknown and also
reports the database-lifetime counters already retained in the raw snapshots.
These include diagnostic/background work, so their scope is explicit and both
sides use the same isolated capture protocol. Stable per-session samples remain
the proof for zero read rollbacks. The raw reference and its original report
are preserved; the corrected reference report is under
`.evidence/oracle-tuning/handoff-20261117/reference-report/` in the step-3 worktree.

The outbox benchmark's completion observer now has a distinct SQL cursor from
the relay's backlog sampler. Its query body and polling cadence are unchanged;
both cursors are warmed before the timed window. A populated diagnostic proves
that observer calls are separate and the production sampler stays within its
timer bound. Independent review found no remaining measurement issues.

Future fresh seeds can pin their business clock with `TUNE_SEED_AT`, so captures
that cross midnight retain the reference's calendar window. A backdated small
seed persisted the requested dates and passed consistency checks. The original step-4 source snapshot is preserved; future snapshots carry these
harness additions.

## Prepared subsequent candidates

- **Completion-time customer activity:** authentication reads only; a guarded
  token/session touch runs immediately before the request completion commit.
  A successful completion clears pending activity. Rollback, failed touches and
  failed commits retain it for completion recovery. Tests cover post-authentication
  rejection, replay, concurrent use, out-of-order completion, logout, password
  reset and bounded-lock retry in backfill and live clock modes.
- **History and balance:** the owned-account lookup finds sequence bounds through
  the time index, then the sequence index drives pagination with exact time,
  direction and cursor predicates retained. This lookup avoids unnecessary
  balance detail work in history. Only the bounded history page uses explicit
  row prefetch to avoid resuming its index scan partway through a page; other
  queries retain the driver's defaults. The latest business date uses an index-only
  minimum/maximum access. Interleaved timestamp/sequence fixtures and runtime
  plan checks exercise both cursor directions and old/recent windows.
  A small populated diagnostic preserved results and found no increase in page
  reads after this revision. Default-population performance acceptance remains
  pending.
- **Posting:** recipient registration is read before acquiring account locks.
  The daily-total merge types its input with `TRUNC(datetime)`, preserving the
  original date value while keeping conversion off the indexed date column.
  The stored-date and index-predicate witness also runs with a non-Gregorian
  session calendar. Oracle documents that this function returns a Gregorian
  `DATE` independently of `NLS_CALENDAR` in the
  [19c SQL Reference](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/TRUNC-date.html).
- **Outbox:** an owned timer samples backlog independently of delivery backoff;
  the oldest-event expression uses index-only minimum/maximum access. The relay's
  ordering, batch size and delete-after-delivery behavior are unchanged. Runtime
  metric, index-plan, Mongo outage/recovery and observability checks passed.
- **Cold paths:** login failure updates return their count; ATM locking reads
  return the fields previously reread separately. Credential locks remain on
  credentials only. Daily reconciliation computes violations once, counts empty
  malformed transfers and derives each account's minimum detail from those rows.
- **Index groups A/B/C:** active-payee predicates match their unique index;
  password reset uses a customer/reset-ID index and preserves the scalar
  latest-ID access; the audited unused request lookup index is removed; daily
  close uses a compressed date/account unique key. Original definitions are
  recorded in forward migration headers. Data-filled migration and fresh-schema
  definition/access-plan witnesses passed.

These candidates remain isolated from the fixed step-3 measurement tree.
No subsequent step is accepted until its approved populated comparison passes.
Stage-specific functional logs and candidate snapshots are under
`.evidence/handoff/step4/` through `.evidence/handoff/step9/` in the staging worktree.

## Step 5 disposition (2026-11-18)

The full populated comparison completed but failed the hot-account Busy
criterion. OCI functional verification passed independently. Short paired
captures on isolated copies of the verified Data Pump restore did not
reproduce the full-capture failure. One full-length, isolated workload window
did show a Busy burst without prior transfer workloads. Its logs showed both
account row-lock and pool-acquisition timeouts, with additional responses
unattributed. A further instrumented window saw no Busy responses despite
observable container CPU throttling. No measurement establishes an initiating
redo, CPU, or storage bottleneck, or attributes the failure to a step-5 code
change.

Per the owner's direction, the root-cause investigation is deferred. The
failed comparison remains open for acceptance. The approved criteria and
step-6 proof requirements still apply.

Astra and Argon independently identified the prepared step-6 posting change
as the scoped mitigation to evaluate. The recipient-registration lookup
moves before account locking on the same database lease. This removes a
lookup from the lock-held portion of posting. Its effect on Busy responses
and lease duration remains unverified.

Local evidence under `.evidence/handoff/reliability/`:

- `stage5-resume-20261117T235708Z/comparison.txt` and `native-evidence.json`
- `hot32-crossover-20261118T022728Z/summary.json`
- `hot32-crossover-full-window-20261118T025149Z/summary.json`
- `hot32-telemetry-20261118T050810Z/telemetry-summary.json`

## Step 6 hot-account diagnostic (2026-11-18)

The exact step-6 candidate ran both hot-transfer concurrency settings on
the Mac mini, starting each run on a new copy of the verified, read-only
Data Pump restore. Both diagnostic windows completed without Busy responses
or unexpected errors. The run logged Oracle waits and Colima CPU and block
device counters. The source inventory matched before and after the test.

The isolated hot-transfer diagnostic evidence is in
`.evidence/handoff/reliability/stage6-hot-diagnostic-20261118T051730Z/`.

The full step-6 capture then passed the unchanged comparison against step 2.
It used a pristine clone of the verified dump on the same Mac mini and
container limits. The existing capture functions ran all workload windows,
plan collection and coverage against the isolated PDB. The source inventory
remained unchanged. The comparison verified that transfer throughput and
hot-account Busy outcomes satisfied their reference criteria and that the
registration lookup moves before account locks. Exact-source OCI full
verification passed independently. This result records step 6 and leaves
the earlier step-5 disposition in place.

See `.evidence/handoff/reliability/stage6-full-20261118T053948Z/` for the
comparison, OCI evidence and pointer to raw capture files.

## Step 7 outbox comparison (2026-11-18)

The full populated capture and exact-source OCI verification completed.
The relay sampler met the approved cadence and the ordinary outbox
throughput criterion failed. The long-backlog diagnostic showed a faster
drain. It does not substitute for the ordinary-workload criterion.

The unchanged relay batch SQL shows ordered OUTBOX_PK access with
WINDOW NOSORT STOPKEY in the step-2 and original-PDB step-5 plan probes,
and TABLE ACCESS FULL with WINDOW SORT PUSHED RANK in the step-6 and step-7
restored-PDB probes. The timed SQL deltas show increased logical reads for
the same relay executions and rows in both restored captures. Step 7
reduced the sampler's logical work. The recorded probes were separate from
the timed windows, so the timed cursor plan and the contribution to overall
drain time still need to be established.

See `.evidence/handoff/reliability/stage7-full-20261118T080938Z/` for the
comparison, OCI logs and pointer to the raw capture.

An Oracle-only replay of the production batch cursor in an isolated PDB
held the restored-PDB optimizer statistics constant and changed only
whether the first execution saw an empty or populated outbox. Both
ordered drains completed, with the same full-scan and pushed-rank plan
and comparable logical reads. First-parse timing alone did not restore
indexed access under those statistics. The replay does not include MongoDB
delivery or application completion polling. The production SQL and the
full comparison verdict remain as recorded above. See
`.evidence/handoff/reliability/stage7-outbox-parse-20261118T114442Z/oracle-replay-exact.log`.

In the same isolated PDB, gathering table and index statistics after
loading the outbox restored ordered OUTBOX_PK access with WINDOW NOSORT
STOPKEY for the same production SQL ID. The ordered drain completed with
lower logical work and lower Oracle-only elapsed time. This establishes
that populated statistics can recover the access path in the replay.
End-to-end outbox performance under an approved, comparable capture
remains unverified. See
`.evidence/handoff/reliability/stage7-outbox-loaded-stats-20261118T114700Z/oracle-replay-loaded-stats.log`.

## Step 8 cold paths (2026-11-18)

The immutable step-8 candidate completed the full populated Mac mini
capture on a fresh clone of the verified seed. The unchanged comparison
passed, including removal of redundant SQL and a single executed daily
reconciliation grouping operator. The exact-source OCI evidence contains
a complete passing native suite, and the daily reconciliation plan was
independently rechecked on OCI. The source inventory matched after the run.

See `.evidence/handoff/reliability/stage8-full-20261118T115233Z/` for the
Mac result and comparison, `.evidence/handoff/step8/functional-evidence-cachefix-v2/`
for OCI checks, and `.evidence/handoff/step8/coverage-recheck-20261118T114841Z/`
for the OCI plan recheck.

## Review of steps 1–8 (2026-11-18)

| Step | Result |
| --- | --- |
| 1. Measurement | The populated baseline identified commit waits and per-request database overhead as the first targets. |
| 2. Harness | The populated reference capture established the workload, resource limits, and counter comparisons. |
| 3. Session overhead | The populated comparison and OCI checks passed. |
| 4. Completion commit | The initial stale-build capture was discarded. The replacement comparison passed after build provenance was verified. |
| 5. History and balance | The full comparison failed the hot-account Busy criterion. Follow-up checks did not establish a root cause. |
| 6. Posting | The full comparison and OCI checks passed. |
| 7. Outbox | Sampler cadence passed and ordinary outbox throughput failed. Populated statistics recovered indexed access in the Oracle-only replay. |
| 8. Cold paths | The full comparison and OCI checks passed. The daily reconciliation plan executed one aggregation. |

Astra, Argon, and NotebookLM independently reviewed the summary. Steps 5
and 7 retain their recorded failed comparisons. The Oracle-only replay
established plan recovery under the tested statistics. The timed application
cursor and end-to-end MongoDB delivery under that plan remain unverified.
Steps 9 and 10 use the original criteria. The fixed-period generator
backfill remains deferred and N-4 remains unproven.

## Steps 9–10 final candidate verification (2026-11-19)

The populated-schema migration witness applied groups A/B/C to the restored
Mac mini seed, preserved records and balances, and found no pending migration
on a second invocation. Fresh OCI migrations and the index access tests
passed. Group D remains outside the candidate because its conditional storage
proof is absent.

The Mac mini ran the complete populated capture and SQL coverage on the
same pinned source used for final OCI verification. This capture is the
step-9 comparison and the final-source Mac evidence for step 10. The
unchanged comparison passed the hot-account Busy check and failed the
ordinary outbox throughput check. The remaining workload comparisons passed.

The exact-source OCI repository suite, fresh migrations, and end-to-end
verification passed. The final functional evidence is complete. Performance
acceptance remains open because the final capture failed and the earlier
step-5 and step-7 comparison verdicts remain open. The separate generator
backfill remains deferred.

See `.evidence/handoff/reliability/stage9-full-20261118T141535Z/` for the
populated migration result, final OCI evidence, capture pointer, and
comparison.

## Final campaign review (2026-11-19)

Astra, Argon, and NotebookLM reviewed steps 1–10. The final-source
performance comparison failed the ordinary outbox criterion. Step 7
also failed its own improvement criterion. Step 5 retains its failure
record, and the final candidate passed the hot-account Busy criterion.

Astra and Argon accepted the pinned Mac capture as evidence of one
execution against the final candidate for steps 9 and 10. NotebookLM
flagged the lack of independent repeatability evidence. The capture
demonstrates one execution and its failing result. The approved plan
specifies no independent second capture on identical final source.

The Oracle-only statistics replay recovered ordered primary-key access.
A controlled application-level outbox comparison would establish whether
that recovery carries through MongoDB delivery. The approved full
comparison and its original thresholds still govern acceptance. The
card-foundation migration-number overlap remains open.

## Remaining acceptance

- Resolve the recorded step-5 and step-7 failures and final-source outbox
  throughput failure under the approved criteria before accepting the change.
- Resolve any migration-number overlap with the separately prepared card
  foundation branch according to merge order before opening the final PR.
- Group D, an IOT and additional maturity/outbox index changes remain
  conditional on their required measured proof.

## Owner-directed verification exception

On 2026-11-17 the owner explicitly instructed the coordinator to skip the
fixed-period generator backfill throughput proof and continue the remaining
tuning work. That step-10 proof is omitted, not passed; this change therefore
does not establish N-4. The existing three-year SQL seed and its consistency
checks remain required and are distinct from this omitted proof. Historical
session review confirmed that the SQL population size was agreed separately;
the generator throughput proof was added to the tuning plan before its driver
was implemented. The current generator has a serial normal-activity driver
and no dedicated backfill entry point. This exception does not waive
the remaining populated comparisons or the registered full verification.

## Build freshness recovery

The first step-4 capture is invalid: Cargo reused the preceding candidate's
release executable after a worktree switch preserved older source timestamps.
Its raw evidence is preserved under `.evidence/handoff/step4/`; it is not an
accepted performance comparison.

The owner approved a source-aware build repair. Rust runners now use a separate
guarded target cache, fingerprint compiler inputs, and hold a cache-local lock
through cleaning, compilation and test execution. Trust is invalidated before
commands and restored atomically only after success with unchanged inputs.
Cleaning affects the bank package in the matching profile; dependency and legacy
caches are retained. Runtime image builds separately lock their named cache and
invalidate the release package. Both runners mount the same read-only contracts.

Container execution separates creation from attachment. Cancellation waits for
signal-shielded creation to publish its immutable container ID, removes that
container, and confirms removal before returning. Failed removals are retried.
The caller's exit status and existing signal traps are preserved.

The coordinator reran real-Cargo old-timestamp, profile-interleaving, dependency
reuse, failed/cancelled command, concurrency and contracts witnesses. BuildKit's
actual build stage passed an old-timestamp source-switch witness; removing its
package clean reproduced the stale executable. Container regressions cover normal
failure, timeout, cancellation during execution and creation, process-group
cancellation and a failed first removal. An explicit-target/custom-profile probe
and a source change during a successful command were also checked. Evidence is
under `.evidence/handoff/cache-*`. Astra and Hydrogen independently reviewed the
repair; their findings and corrections are recorded there. The replacement step-4 comparison and final-source OCI verification
subsequently passed. See the step summaries above for the Mac verdicts.
