# Reviewer reports (2026-11-15)

Raw read-only reviews behind plan.md. Not approved artifacts.

## Fable (Claude, repository read access)

### Oracle tuning review: Saltation bank schema and SQL

Nothing was executed, so every finding is from reading the DDL, SQL and Rust callers; each names the plan or statistic that would confirm it. Paths are under `/Users/ckh/repos/saltation/bank/`.

#### 1. Ranked findings

1. **DATE columns compared with TIMESTAMP binds** (`src/shared/db/values.rs`, `NaiveDate` binds as `DB_TYPE_TIMESTAMP`). Affects `ledger/sql/lock_daily.sql`, `ledger/sql/add_daily.sql` and `deposits/sql/candidates.sql`.
   - Problem: Oracle converts the column, not the bind (`INTERNAL_FUNCTION(business_date) = :b`). The daily-total PK is then accessed on `account_id` only and filters every day of that account's history, twice per transfer, under the account lock. `deposit_detail_maturity_ix` cannot range-scan.
   - Change: `business_date = CAST(:business_date AS DATE)` in the three statements (and in the MERGE's `USING` select), or bind `NaiveDate` as DATE in the adapter.
   - Effect: unique scan instead of a range scan that grows with history. It is a prerequisite for findings 6 and 7.
   - Confidence: medium-high. Confirm with `INTERNAL_FUNCTION` in the DBMS_XPLAN predicate section.

2. **Outbox backlog probe runs before every 500-row batch** (`shared/outbox/sql/select_backlog.sql`, called from the loop in `features/outbox_relay/mod.rs`; `outbox_created_ix` in `migrations/0011`).
   - Problem: `COUNT(*)` plus `MIN(created_at)` is a full index scan per batch, so cost is proportional to the backlog exactly when the relay is behind. `outbox_created_ix` exists only for this and is maintained on every insert and delete of the highest-churn table.
   - Change: `DROP INDEX outbox_created_ix`. Take the oldest age from the first row of `select_batch` (add `created_at` to it, or decode the ULID time). Sample `COUNT(*)` on a timer (say every 5 s) with a fast full scan of `outbox_pk`.
   - Effect: one fewer index maintained per API request, and relay cost no longer grows with backlog.
   - Confidence: high.

3. **Every authenticated request is its own write transaction** (`shared/auth/mod.rs`: `select_token`, `extend_token`, `touch_session`, commit).
   - Problem: four round trips, two updates and a synchronous log write, even for read-only inquiries, before the handler takes a second pool session.
   - Change: put both updates in one anonymous PL/SQL block ending in `COMMIT WRITE BATCH NOWAIT` (touch-only data).
   - Effect: two round trips and one log-sync wait fewer per request across about 9.5M requests a year.
   - Confidence: medium-high. Confirm with the `log file sync` share in AWR.
   - Skipping the update when expiry moved less than about a minute would save more, but it loosens the 30-minute sliding rule, so that is an owner decision.

4. **Too many round trips while account locks are held** (`shared/ledger/mod.rs::post`, `features/transfers/mod.rs::attempt`).
   - Problem: about 9 to 10 calls sit between the lock and the commit: `lock_daily`, `registered`, `insert_transfer`, `insert_row`, `update_account`, `add_daily`, `finish_request`, `session_device`, `insert_event`, commit. `lock_accounts` is slowest because waiters queue behind that hold time, not because of its plan.
   - Changes:
     - Run `transfers.registered` before `lock_accounts`; it does not depend on the lock.
     - Pass the already-known `AuthCustomer.device_id` into the event so `outbox.session_device` is not issued.
     - Post transfer, rows, account update, daily MERGE, `finish_request` and the outbox insert in one static PL/SQL block.
   - Effect: lock hold drops from about 9 round trips to 2 or 3, which should lift the 320/s ceiling and p95. Locking order, the single transaction and insert-only rows are unchanged.
   - Confidence: medium. Confirm with per-statement timings against `enq: TX - row lock contention`.

5. **History pagination sorts the whole window on every page** (`features/accounts/sql/select_transactions_{asc,desc}.sql`).
   - Problem: the hint forces `account_transaction_time_ix`, but the order is `account_sequence`. Every row in the window gets a table visit, then `SORT ORDER BY STOPKEY`.
   - Change: drive by the sequence key so the scan stops at the page size: `/*+ INDEX_DESC(tx account_transaction_seq_uk) */ ... WHERE account_id = :a AND account_sequence < :before AND occurred_at >= :from AND occurred_at < :to ORDER BY account_sequence DESC FETCH FIRST :n ROWS ONLY` (`INDEX` for ascending).
   - Effect: about 26 row visits for recent windows instead of the whole window. Old windows on long-lived accounts walk back from the newest row, so check the plan for both shapes.
   - Also: this handler runs `select_balance` (six-table join plus a MAX subquery) and uses only `account_id`, `account_no` and `balance`. Give it a two-table lookup.
   - Confidence: medium-high. Matters at hundreds of rows or more per account.

6. **`daily_close` is keyed for the wrong access** (`migrations/0010`).
   - Problem: one row per open account per business day makes this the largest table by rows (about 10M a year). PK `(account_id, business_date)` scatters each day's inserts over every leaf block, and `daily_close_date_ix` is a second full-size index.
   - Change: PK `(business_date, account_id) COMPRESS 1` and drop `daily_close_date_ix`; optionally `ORGANIZATION INDEX`. All five daily_close statements stay index-driven; `report.sql` then reads `verified` from the table, about 30k clustered rows once a day.
   - Effect: append-only inserts and roughly half the storage.
   - Confidence: high. Matters after months of data.

7. **`daily_account_total`** (`migrations/0004`).
   - Problem: it is a heap plus an account-leading PK, used only for PK lookups.
   - Change: `ORGANIZATION INDEX` with PK `(business_date, account_id) COMPRESS 1`.
   - Effect: one segment, no table hop for `lock_daily` or the MERGE, and only today's leaf blocks are hot.
   - Confidence: medium. Do it only after finding 1, or it becomes a full scan.

8. **Redundant or unused indexes on insert-heavy tables.**
   - `account_dormancy_ix`: rewritten on each customer-initiated post that advances `last_activity_at`, under the lock, to serve a once-per-business-day scan of a table of about 500 blocks. Drop it and full-scan. Confidence: high.
   - `account_transaction_date_ix`: used only for `MAX(business_date)` in `select_balance.sql`. Replace with `TO_CHAR(MAX(SYS_EXTRACT_UTC(tx.occurred_at)) + INTERVAL '9' HOUR, 'YYYYMMDD')` on `time_ix`, then drop it (6 indexes down to 5 on the biggest table). Confidence: medium. Confirm the plan shows `INDEX RANGE SCAN (MIN/MAX)`.
   - `transaction_request_btid_ix` and `login_attempt_phone_ix`: no statement under `bank/src` uses them. Drop them unless an external reader does.

9. **`transaction_request` rows grow after insert** (`*/sql/insert_request.sql`, then `finish_request.sql`).
   - Problem: `response_json` goes from NULL to several hundred bytes at default PCTFREE 10, which invites row migration.
   - Change: `ALTER TABLE transaction_request PCTFREE 40` before the backfill (existing rows need a MOVE).
   - Confidence: medium. Confirm with `table fetch continued row` or `ANALYZE ... LIST CHAINED ROWS`.
   - The `IS JSON` checks here and on `outbox.event_json` re-parse bank-generated JSON on every write. Dropping them saves CPU (low confidence on size).

10. **Storage and contention hygiene** (low to medium).
    - `COMPRESS 1` on `account_transaction_seq_uk`, `account_transaction_time_ix` and `account_status_history_ix`. Basic prefix compression needs no extra option and helps the 70 GB budget.
    - `ALTER TABLE account PCTFREE 20 INITRANS 8` for hot balance rows. Confirm with `enq: TX - allocate ITL entry`.
    - `outbox PCTFREE 0`, and a periodic `ALTER INDEX outbox_pk COALESCE` at rollover, because the left edge of a queue index fills with emptied leaves that `select_batch` must walk. Confirm with rising gets per execution.
    - `transfers/sql/registered.sql`: repeat the `payee_active_uk` CASE expressions to get an index-only unique probe.
    - `password_reset_customer_ix`: change to `(customer_id, reset_id)` so `MAX(reset_id)` is a min/max scan.

11. **Partitioning (separately licensed option): not recommended now.** With no purge it buys little. Interval-partitioning `account_transaction` by `business_date` is only worth it if, at 50M+ rows, AWR shows physical reads on the account-leading indexes; `seq_uk` would have to stay global.

#### 2. Fine as is

- **`lock_accounts.sql`**: PK IN-list, ascending order, `WAIT 1`; the plan is not the problem.
- **`add_daily` MERGE**: one round trip, serialised by the account lock.
- **Array DML**: used for ledger rows, account updates and outbox deletes; deleting by explicit ID is correct.
- **Identity caches**: `CACHE 10000` / `1000` with NOORDER.
- **Right-growing keys** (ULID and identity PKs, `*_occurred_ix`): with 12 sessions on one instance, do not use reverse-key or hash partitioning. `outbox_pk` and the `occurred_at` indexes need range scans.
- **Foreign keys**: parents are never deleted; the only updated parent key (`account.account_no`) has both children indexed.
- **TSTZ predicates**: the TIMESTAMP bind is converted, not the column.
- **Other access paths**: idempotency by `transaction_request_idem_uk`, the function-based ATM and payee unique indexes, and `sms_outbox_phone_ix`.
- **Analyst search**: no such SQL exists under `bank/src`, so there is nothing to tune yet.

## Astra (gpt-6-astra, medium)
Read-only review completed; no files changed and no database commands run. These are source-based findings, not measured execution plans. Paths below are relative to `bank/`.

1. **Shorten account-lock duration before adding indexes — medium confidence.** `src/shared/ledger/mod.rs`, `ledger.lock_accounts`, and `src/features/transfers/mod.rs`: the lock query already targets at most two PK rows; its elapsed time may primarily reflect waiting for earlier transactions.
   Consolidate posting into a static bound PL/SQL block containing transfer `INSERT … RETURNING`, ledger inserts, balance updates and daily-total changes; batch adjacent outbox inserts. Preserve ascending locks and the final atomic commit. Existing ledger array DML already helps. Confirm with blocking-session/TX waits versus CPU and buffer gets; another account index cannot cure same-row contention.

2. **Remove repeated full-backlog counting from delivery — high confidence.** `src/features/outbox_relay/mod.rs`, `src/shared/outbox/sql/select_backlog.sql`: every 500-row batch first executes `COUNT(*)` across the remaining outbox. Draining a large backlog repeatedly scans it.
   Sample backlog metrics on a timer, independently of delivery; retain exact counts at that cadence. This reduces recovery work substantially **when backlog is large**. Keep `outbox_created_ix` initially: it supports the count/oldest-time query without reading JSON.

3. **Correct DATE bind mismatches — high confidence in mismatch; medium in performance impact.** `src/shared/db/values.rs`, `ledger/{lock_daily,add_daily}.sql`, `deposits/candidates.sql`: `NaiveDate` binds as TIMESTAMP although indexed columns are DATE.
   Bind native DATE, or use `business_date = CAST(:business_date AS DATE)` and cast the MERGE source/date cutoff likewise. This avoids column-side promotion potentially weakening access predicates. Check actual predicate information for `INTERNAL_FUNCTION` and whether both daily-total PK columns are access predicates. TSTZ comparisons are different: Oracle promotes the TIMESTAMP bind using the session zone. [Oracle datetime conversions](https://docs.oracle.com/en/database/oracle/oracle-database/19/nlspg/datetime-data-types-and-time-zone-support.html)

4. **Stop forcing one history access path — medium confidence.** `src/features/accounts/sql/select_transactions_{asc,desc}.sql`: `(account_id, occurred_at, account_sequence)` restricts dates, but cannot provide sequence ordering across a time range; later-page sequence conditions also generally remain filters.
   Remove the forced `INDEX` hint and compare against existing `(account_id, account_sequence)` forward/backward scans with early stop. Retain time-index plans for narrow historical windows. Use separate static all/in/out variants if the direction OR produces poor estimates. Benefit grows with **rows per account/window**; compare buffers and sort work for first/deep pages. Do not change cursor ordering to time without changing its contract.

5. **Exploit the existing active-payee unique index — high confidence.** `src/features/transfers/sql/registered.sql`, migration `0008`: ordinary column predicates do not directly match the CASE expressions in `payee_active_uk`, leaving customer-history filtering inside the account-lock interval.
   Rewrite predicates as `CASE WHEN deleted_at IS NULL THEN customer_id END = :customer_id`, and equivalent expressions for `bank_code` and `account_no`. All three then match the existing unique key; no new index is needed. Largest benefit comes with accumulated deleted payees.

6. **Remove an apparently unused write-heavy index — medium confidence.** Migration `0006`, `transaction_request_btid_ix`: no supplied query searches `bank_tran_id`; even `transfers.select_result` uses `(idem_scope, idem_key)`.
   After checking external/reporting SQL, use `DROP INDEX transaction_request_btid_ix`. This saves insert redo, space and leaf maintenance at **millions of requests**. Making it invisible tests read dependence but does not remove write costs. Do not drop the ledger date/time indexes merely because their prefixes overlap: their access paths differ.

7. **Index only outstanding deposit maturities — high confidence, chiefly at large history sizes.** Migration `0009`, `deposits/candidates.sql`: `(maturity_date, closed_at)` scans the growing historical maturity range, with closed-state filtering after the leading range.
   Replace it with `CREATE INDEX deposit_open_due_ix ON deposit_detail (CASE WHEN closed_at IS NULL THEN maturity_date END)` and query that exact expression `<= CAST(:business_date AS DATE)`. Closed rows disappear from this index. The final account-ID sort remains necessary.

8. **Fix newest-reset lookup ordering — high confidence, modest overall impact.** Migration `0008`, `password_reset/{peek_code,select_code}.sql`: queries seek `MAX(reset_id)` per customer, while the index orders by `issued_at`.
   Replace `password_reset_customer_ix` with an index on `(customer_id, reset_id)` after checking reporting dependencies. This enables an efficient maximum lookup and preserves the customer FK prefix; benefit grows with resets per customer.

9. **Remove redundant statement round trips — high confidence.** `features/auth/sql/{record_failure,select_failures}.sql`: append `RETURNING failed_count, locked_until INTO :failed, :locked` to the update and remove the following select.
   In `features/atm/sql/{lock_session,session}.sql` and callers, return the required session columns from the locking select instead of immediately querying them again; validate post-wait state behavior. In `shared/auth`, bundle token-expiry and session-touch updates in one static PL/SQL block. Preserve required locks and expiry semantics.

10. **Address physical contention only after identifying its type — medium confidence.** Migrations `0003/0004`, hot `account`, `daily_account_total`, and request rows: defaults may need adjustment, but slow locking alone does not establish block contention.
    If ITL waits are measured, trial `ALTER TABLE account INITRANS 8` and equivalent settings on affected totals/index segments; existing full blocks may require reorganization. For request-row migration caused by adding `response_json`, trial higher PCTFREE or a separate response table. Neither fixes same-row TX waits; increased free space consumes the 70 GB budget. [Oracle physical attributes](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/physical_attributes_clause.html)

11. **Treat right-growing index remedies as conditional — low confidence without waits.** Migrations `0002/0004/0011`: identity/ULID PKs can concentrate inserts. For confirmed leaf contention on equality-only PKs such as `account_transaction(tran_id)`, trial `ALTER INDEX <actual_pk_index> REBUILD REVERSE`.
    Alternatively trial a global hash-partitioned PK index; **this requires the separately licensed Partitioning option**. Neither cures hot account rows. Never reverse the outbox PK, sequence-history index or date indexes: ordered/range access matters. Identity caches are already generous; retain NOORDER. [Oracle index behavior](https://docs.oracle.com/en/database/oracle/oracle-database/19/cncpt/indexes-and-index-organized-tables.html), [licensing](https://docs.oracle.com/en/database/oracle/oracle-database/19/dblic/Licensing-Information.html)

12. **Reduce reconciliation work and budget its retained rows — medium confidence.** `features/daily_close/sql/{transfers,count_transfers}.sql` computes the same transfer-shape aggregation twice under the rollover barrier.
    Materialize violations once in a transaction-private temporary table, then count and update from it; preserve empty-transfer violations. Also budget `daily_close`: 20,000 continuously open accounts produce about **7.3 million rows/year**, before additional accounts. Trial basic prefix compression on repeated-account ledger/daily indexes and lower heap PCTFREE for immutable ledger rows; measure space, CPU and ITL effects before adopting either.

13. **Date partitioning is a scale option, not an immediate throughput fix — medium confidence.** Migrations `0004/0010/0013`: at tens of millions of rows, consider monthly `PARTITION BY RANGE(business_date) INTERVAL(NUMTOYMINTERVAL(1,'MONTH'))` for `account_transaction` and `daily_close`.
    Add matching date predicates alongside exact timestamp bounds for pruning. Retain global uniqueness for `tran_id` and `(account_id, account_sequence)`; adding the date to unique keys would weaken existing semantics. `transfer`/`transaction_request` need a suitable DATE key first: their TSTZ columns cannot directly be partition keys in 19c. **Partitioning is separately licensed**; retention means no partition drops. Keep the promptly drained outbox unpartitioned initially. [Oracle partition restrictions](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/CREATE-TABLE.html)

**Things that look fine, or should remain conditional**

- Preserve ascending account locking, `WAIT 1`, atomic posting, immutable transaction rows, existing `RETURNING` usage and array DML. `lock_daily` is already a nonlocking PK aggregate; its MERGE is serialized by the account lock. An application-side update-then-insert adds a round trip on first use; no demonstrated improvement.
- Preserve outbox event-ID ordering, batches of 500 and deletion by delivered IDs. Range deletion or `SKIP LOCKED` consumers could violate delivery semantics. Do not rebuild churned indexes routinely without measured bloat/access degradation.
- Missing FK indexes on `account_transaction(request_id/reversal_of)` and detail `linked_account_id` are not automatically locking defects: relevant parent keys are not deleted or changed. Add them for demonstrated joins or changed parent-key operations, not reflexively. [Oracle FK locking](https://docs.oracle.com/en/database/oracle/oracle-database/19/cncpt/data-concurrency-and-consistency.html)
- Analyst SQL is absent from this checkout. Phone/customer-number equality already has unique indexes; a name index helps equality/prefix search, not arbitrary `%substring%`. For future transaction filters, start with existing account/date access; add `(account_id, channel, occurred_at, account_sequence)` only if channel selectivity warrants its write cost. Avoid speculative wide covering indexes.
- Keep exact NUMBER money and business-time semantics. JSON is currently VARCHAR2, not LOB: no SecureFiles tuning applies. IOT conversion of the wide, multiply indexed ledger/outbox is unproven; narrow daily totals are a more plausible benchmark candidate. Nullable-column reordering offers only minor savings and does not justify rebuilding existing tables alone.
- Before adopting conditional changes, obtain actual cursor row counts, buffers, access/filter predicates, sorts and wait events with representative binds and multi-year data. Refresh optimizer statistics during large backfills; the supplied peak rates alone do not establish sustained mixed-workload N-4 compliance.
tokens used
85,952
Read-only review completed; no files changed and no database commands run. These are source-based findings, not measured execution plans. Paths below are relative to `bank/`.

1. **Shorten account-lock duration before adding indexes — medium confidence.** `src/shared/ledger/mod.rs`, `ledger.lock_accounts`, and `src/features/transfers/mod.rs`: the lock query already targets at most two PK rows; its elapsed time may primarily reflect waiting for earlier transactions.
   Consolidate posting into a static bound PL/SQL block containing transfer `INSERT … RETURNING`, ledger inserts, balance updates and daily-total changes; batch adjacent outbox inserts. Preserve ascending locks and the final atomic commit. Existing ledger array DML already helps. Confirm with blocking-session/TX waits versus CPU and buffer gets; another account index cannot cure same-row contention.

2. **Remove repeated full-backlog counting from delivery — high confidence.** `src/features/outbox_relay/mod.rs`, `src/shared/outbox/sql/select_backlog.sql`: every 500-row batch first executes `COUNT(*)` across the remaining outbox. Draining a large backlog repeatedly scans it.
   Sample backlog metrics on a timer, independently of delivery; retain exact counts at that cadence. This reduces recovery work substantially **when backlog is large**. Keep `outbox_created_ix` initially: it supports the count/oldest-time query without reading JSON.

3. **Correct DATE bind mismatches — high confidence in mismatch; medium in performance impact.** `src/shared/db/values.rs`, `ledger/{lock_daily,add_daily}.sql`, `deposits/candidates.sql`: `NaiveDate` binds as TIMESTAMP although indexed columns are DATE.
   Bind native DATE, or use `business_date = CAST(:business_date AS DATE)` and cast the MERGE source/date cutoff likewise. This avoids column-side promotion potentially weakening access predicates. Check actual predicate information for `INTERNAL_FUNCTION` and whether both daily-total PK columns are access predicates. TSTZ comparisons are different: Oracle promotes the TIMESTAMP bind using the session zone. [Oracle datetime conversions](https://docs.oracle.com/en/database/oracle/oracle-database/19/nlspg/datetime-data-types-and-time-zone-support.html)

4. **Stop forcing one history access path — medium confidence.** `src/features/accounts/sql/select_transactions_{asc,desc}.sql`: `(account_id, occurred_at, account_sequence)` restricts dates, but cannot provide sequence ordering across a time range; later-page sequence conditions also generally remain filters.
   Remove the forced `INDEX` hint and compare against existing `(account_id, account_sequence)` forward/backward scans with early stop. Retain time-index plans for narrow historical windows. Use separate static all/in/out variants if the direction OR produces poor estimates. Benefit grows with **rows per account/window**; compare buffers and sort work for first/deep pages. Do not change cursor ordering to time without changing its contract.

5. **Exploit the existing active-payee unique index — high confidence.** `src/features/transfers/sql/registered.sql`, migration `0008`: ordinary column predicates do not directly match the CASE expressions in `payee_active_uk`, leaving customer-history filtering inside the account-lock interval.
   Rewrite predicates as `CASE WHEN deleted_at IS NULL THEN customer_id END = :customer_id`, and equivalent expressions for `bank_code` and `account_no`. All three then match the existing unique key; no new index is needed. Largest benefit comes with accumulated deleted payees.

6. **Remove an apparently unused write-heavy index — medium confidence.** Migration `0006`, `transaction_request_btid_ix`: no supplied query searches `bank_tran_id`; even `transfers.select_result` uses `(idem_scope, idem_key)`.
   After checking external/reporting SQL, use `DROP INDEX transaction_request_btid_ix`. This saves insert redo, space and leaf maintenance at **millions of requests**. Making it invisible tests read dependence but does not remove write costs. Do not drop the ledger date/time indexes merely because their prefixes overlap: their access paths differ.

7. **Index only outstanding deposit maturities — high confidence, chiefly at large history sizes.** Migration `0009`, `deposits/candidates.sql`: `(maturity_date, closed_at)` scans the growing historical maturity range, with closed-state filtering after the leading range.
   Replace it with `CREATE INDEX deposit_open_due_ix ON deposit_detail (CASE WHEN closed_at IS NULL THEN maturity_date END)` and query that exact expression `<= CAST(:business_date AS DATE)`. Closed rows disappear from this index. The final account-ID sort remains necessary.

8. **Fix newest-reset lookup ordering — high confidence, modest overall impact.** Migration `0008`, `password_reset/{peek_code,select_code}.sql`: queries seek `MAX(reset_id)` per customer, while the index orders by `issued_at`.
   Replace `password_reset_customer_ix` with an index on `(customer_id, reset_id)` after checking reporting dependencies. This enables an efficient maximum lookup and preserves the customer FK prefix; benefit grows with resets per customer.

9. **Remove redundant statement round trips — high confidence.** `features/auth/sql/{record_failure,select_failures}.sql`: append `RETURNING failed_count, locked_until INTO :failed, :locked` to the update and remove the following select.
   In `features/atm/sql/{lock_session,session}.sql` and callers, return the required session columns from the locking select instead of immediately querying them again; validate post-wait state behavior. In `shared/auth`, bundle token-expiry and session-touch updates in one static PL/SQL block. Preserve required locks and expiry semantics.

10. **Address physical contention only after identifying its type — medium confidence.** Migrations `0003/0004`, hot `account`, `daily_account_total`, and request rows: defaults may need adjustment, but slow locking alone does not establish block contention.
    If ITL waits are measured, trial `ALTER TABLE account INITRANS 8` and equivalent settings on affected totals/index segments; existing full blocks may require reorganization. For request-row migration caused by adding `response_json`, trial higher PCTFREE or a separate response table. Neither fixes same-row TX waits; increased free space consumes the 70 GB budget. [Oracle physical attributes](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/physical_attributes_clause.html)

11. **Treat right-growing index remedies as conditional — low confidence without waits.** Migrations `0002/0004/0011`: identity/ULID PKs can concentrate inserts. For confirmed leaf contention on equality-only PKs such as `account_transaction(tran_id)`, trial `ALTER INDEX <actual_pk_index> REBUILD REVERSE`.
    Alternatively trial a global hash-partitioned PK index; **this requires the separately licensed Partitioning option**. Neither cures hot account rows. Never reverse the outbox PK, sequence-history index or date indexes: ordered/range access matters. Identity caches are already generous; retain NOORDER. [Oracle index behavior](https://docs.oracle.com/en/database/oracle/oracle-database/19/cncpt/indexes-and-index-organized-tables.html), [licensing](https://docs.oracle.com/en/database/oracle/oracle-database/19/dblic/Licensing-Information.html)

12. **Reduce reconciliation work and budget its retained rows — medium confidence.** `features/daily_close/sql/{transfers,count_transfers}.sql` computes the same transfer-shape aggregation twice under the rollover barrier.
    Materialize violations once in a transaction-private temporary table, then count and update from it; preserve empty-transfer violations. Also budget `daily_close`: 20,000 continuously open accounts produce about **7.3 million rows/year**, before additional accounts. Trial basic prefix compression on repeated-account ledger/daily indexes and lower heap PCTFREE for immutable ledger rows; measure space, CPU and ITL effects before adopting either.

13. **Date partitioning is a scale option, not an immediate throughput fix — medium confidence.** Migrations `0004/0010/0013`: at tens of millions of rows, consider monthly `PARTITION BY RANGE(business_date) INTERVAL(NUMTOYMINTERVAL(1,'MONTH'))` for `account_transaction` and `daily_close`.
    Add matching date predicates alongside exact timestamp bounds for pruning. Retain global uniqueness for `tran_id` and `(account_id, account_sequence)`; adding the date to unique keys would weaken existing semantics. `transfer`/`transaction_request` need a suitable DATE key first: their TSTZ columns cannot directly be partition keys in 19c. **Partitioning is separately licensed**; retention means no partition drops. Keep the promptly drained outbox unpartitioned initially. [Oracle partition restrictions](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/CREATE-TABLE.html)

**Things that look fine, or should remain conditional**

- Preserve ascending account locking, `WAIT 1`, atomic posting, immutable transaction rows, existing `RETURNING` usage and array DML. `lock_daily` is already a nonlocking PK aggregate; its MERGE is serialized by the account lock. An application-side update-then-insert adds a round trip on first use; no demonstrated improvement.
- Preserve outbox event-ID ordering, batches of 500 and deletion by delivered IDs. Range deletion or `SKIP LOCKED` consumers could violate delivery semantics. Do not rebuild churned indexes routinely without measured bloat/access degradation.
- Missing FK indexes on `account_transaction(request_id/reversal_of)` and detail `linked_account_id` are not automatically locking defects: relevant parent keys are not deleted or changed. Add them for demonstrated joins or changed parent-key operations, not reflexively. [Oracle FK locking](https://docs.oracle.com/en/database/oracle/oracle-database/19/cncpt/data-concurrency-and-consistency.html)
- Analyst SQL is absent from this checkout. Phone/customer-number equality already has unique indexes; a name index helps equality/prefix search, not arbitrary `%substring%`. For future transaction filters, start with existing account/date access; add `(account_id, channel, occurred_at, account_sequence)` only if channel selectivity warrants its write cost. Avoid speculative wide covering indexes.
- Keep exact NUMBER money and business-time semantics. JSON is currently VARCHAR2, not LOB: no SecureFiles tuning applies. IOT conversion of the wide, multiply indexed ledger/outbox is unproven; narrow daily totals are a more plausible benchmark candidate. Nullable-column reordering offers only minor savings and does not justify rebuilding existing tables alone.
- Before adopting conditional changes, obtain actual cursor row counts, buffers, access/filter predicates, sorts and wait events with representative binds and multi-year data. Refresh optimizer statistics during large backfills; the supplied peak rates alone do not establish sustained mixed-workload N-4 compliance.

## Hydrogen (gemini-4-hydrogen-high, no repository access)
#### 1. Ranked Performance Findings

1. **Shorten `account` lock hold time (`shared/ledger/sql/lock_accounts.sql`, `lock_daily.sql`, `insert_row.sql`, `update_account.sql`, `add_daily.sql`)**
   - **Problem:** `lock_accounts` (`FOR UPDATE WAIT 1`) reports the highest latency because concurrent transfers on hot accounts queue on `enq: TX - row lock contention`. Calling `lock_daily` separately adds 1–2 extra round trips inside the lock, and separate per-leg writes multiply lock-hold duration (if not already batched in Rust).
   - **Change:** Fold `lock_daily` into `lock_accounts` while locking only `account`:
     ```sql
     SELECT a.account_id, a.balance, a.next_sequence, a.status, a.subtype, a.account_no,
            NVL(d.transfer_out, 0), NVL(d.cash_in, 0), NVL(d.cash_out, 0)
     FROM account a LEFT JOIN daily_account_total d
       ON d.account_id = a.account_id AND d.business_date = :business_date
     WHERE a.account_id IN (:first_id, :second_id) ORDER BY a.account_id FOR UPDATE OF a.balance WAIT 1;
     ```
     Batch 2-leg `insert_row`, `update_account`, `add_daily`, and `outbox/insert_event` via array DML (depends on Rust caller).
   - **Effect / Confidence:** Cuts 1–5 round trips inside the critical section, reducing lock-wait p95. **High** (confirm via ASH/AWR `enq: TX - row lock contention` vs `SQL*Net` wait share).

2. **Eliminate full-window table fetch and sort in history pagination (`accounts/sql/select_transactions_asc.sql`, `select_transactions_desc.sql`, `0004_ledger.sql`)**
   - **Problem:** `/*+ INDEX(tx account_transaction_time_ix) */` forces a range scan of all rows in `[from_time, to_time)`, table lookups for every row, and `SORT ORDER BY STOPKEY` even for page 1 (~530/s).
   - **Change:** Enforce `account_transaction_seq_uk` with a covering non-unique index and hint `INDEX_ASC`/`INDEX_DESC` on it:
     ```sql
     CREATE INDEX account_transaction_seq_ix ON account_transaction (account_id, account_sequence, occurred_at, amount) COMPRESS 1;
     ALTER TABLE account_transaction DROP CONSTRAINT account_transaction_seq_uk;
     ALTER TABLE account_transaction ADD CONSTRAINT account_transaction_seq_uk UNIQUE (account_id, account_sequence) USING INDEX account_transaction_seq_ix;
     ```
     Replace query hint with `/*+ INDEX_ASC(tx account_transaction_seq_ix) */` / `/*+ INDEX_DESC(tx account_transaction_seq_ix) */` (and drop `account_transaction_time_ix` if unused).
   - **Effect / Confidence:** Index-only `NOSORT` + `COUNT STOPKEY`, visiting table blocks only for `:page_size` rows (matters most once accounts accumulate hundreds/thousands of rows). **High** (confirm via `DBMS_XPLAN` `SORT ORDER BY` removal).

3. **Prevent row migration and ITL waits on hot-updated tables (`0003_accounts.sql`, `0004_ledger.sql`)**
   - **Problem:** `transaction_request` inserts `response_json` as `NULL` then updates it to JSON in `finish_request`, overflowing default `PCTFREE 10` and migrating rows. `account` grows `balance`/`next_sequence`/`last_activity_at` from `0`/`NULL` and uses `INITRANS 1` under 32–64 concurrent sessions.
   - **Change:** Set in DDL (or `ALTER TABLE` + `MOVE` for existing rows): `transaction_request` (`PCTFREE 40 INITRANS 16`), `account` (`PCTFREE 25 INITRANS 16`), `daily_account_total` (`PCTFREE 20 INITRANS 16`).
   - **Effect / Confidence:** Eliminates row migration on `finish_request`/`update_account` and ITL contention. **High** (confirm via `USER_TABLES.CHAIN_CNT` and `enq: TX - allocate ITL entry`).

4. **Reduce right-edge index leaf splits and drop unneeded high-churn indexes (`0004_ledger.sql`, `0006_transfer_indexes.sql`, `0011_performance.sql`)**
   - **Problem:** Monotonic PKs (`tran_id`, `transfer_id`, ULID `request_id`) contend on the rightmost B-tree leaf block. `transaction_request_btid_ix` (`bank_tran_id`) is not queried by any production SQL. `account_dormancy_ix` adds a function-based index update to every `account` movement for a once-daily job.
   - **Change:** `ALTER INDEX <sys_pk_idx> REBUILD REVERSE;` on `account_transaction(tran_id)`, `transfer(transfer_id)`, and `transaction_request(request_id)` (lookup names in `USER_INDEXES`; equality-only PKs); `DROP INDEX transaction_request_btid_ix;` and `DROP INDEX account_dormancy_ix;` (let daily `candidates.sql` scan 20k–60k `account` rows).
   - **Effect / Confidence:** Removes 2 hot index writes per request/transfer and cuts `enq: TX - index contention`. **High** (`account_dormancy_ix`, `REVERSE` PKs); **Medium** (`btid_ix`—verify no unlisted caller/test queries `bank_tran_id`).

5. **Prefix-compress indexes and partition historical ledger tables for the 70 GB budget (`0004_ledger.sql`, `0010_rollover.sql`, `0013_close_indexes.sql`)**
   - **Problem:** At 30M–50M+ requests over several years, `transaction_request` and `account_transaction` (6 indexes) can exceed 70 GB, and `daily_close` scans full-day ranges across an ever-larger heap.
   - **Change:** Add `COMPRESS 1` (base EE) to `account_transaction_date_ix`, `transaction_request_idem_uk`, `daily_close_pk`, `daily_account_total_pk`, `auth_session_customer_ix`, `login_attempt_phone_ix`. **With separately licensed EE Partitioning option:** interval-partition `account_transaction` and `transaction_request` monthly by `occurred_at` with `LOCAL` `occurred_ix` (and `GLOBAL` hash-partitioned PKs), then `ALTER TABLE ... MODIFY PARTITION ... COMPRESS BASIC` on closed months.
   - **Effect / Confidence:** ~30–50% space reduction and partition-pruned `daily_close` (only matters at multi-million row counts). **High**.

6. **Control `outbox` FIFO fragmentation and `outbox_created_ix` write cost (`0004_ledger.sql`, `0011_performance.sql`, `shared/outbox/sql/select_backlog.sql`)**
   - **Problem:** Continuous right-edge inserts and left-edge array deletes leave sparse empty leaf blocks in `outbox_pk` and `outbox_created_ix`, slowing `select_batch` (`FETCH FIRST 500`) and `select_backlog`; `outbox_created_ix` adds a 2nd hot-leaf index on every API request.
   - **Change:** Run periodic online `ALTER INDEX outbox_pk COALESCE;` (and `outbox_created_ix`, or `ALTER TABLE outbox SHRINK SPACE COMPACT`); evaluate dropping `outbox_created_ix` if `outbox` stays small / `select_backlog` is rare.
   - **Effect / Confidence:** Prevents scan degradation over long backfills. **Medium** (confirm via `LEAF_BLOCKS` vs `NUM_ROWS` in `USER_INDEXES`).

7. **Eliminate redundant round trips in `atm`/`auth` and double scan in `daily_close` (`atm/sql/*`, `auth/sql/*`, `daily_close/sql/*`)**
   - **Problem:** `atm/lock_session` + `session` read `atm_session_token_uk` twice; `atm/lock_credential` + `credential` read `atm_credential` twice; `auth/record_failure` + `select_failures` do `UPDATE` then `SELECT`; `daily_close/count_transfers` and `transfers` execute the same heavy `shapes`/`violations` join twice.
   - **Change:** Return columns directly in `atm/lock_session` (`FOR UPDATE`) and join `account` in `atm/lock_credential` (`FOR UPDATE OF c.failed_count`); use `UPDATE ... RETURNING failed_count, locked_until INTO ...` in `auth/record_failure`; reuse `daily_close/transfers` results instead of re-running `count_transfers`.
   - **Effect / Confidence:** Saves 1 round trip per ATM op / failed login and halves `daily_close` transfer-check I/O (depends on Rust caller). **High**.

8. **Missing indexes for analyst search/filter workload (spec-driven; SQL not in repo excerpt)**
   - **Problem:** Spec states analysts search customers by name/phone/number and filter transactions by date, channel, direction, and amount, but `customer(name)` is unindexed and `account_transaction_occurred_ix` lacks `channel`/`inout_type`/`amount`.
   - **Change:** Add `CREATE INDEX customer_name_ix ON customer (name);` (or `UPPER(name)` if case-insensitive); add `(occurred_at, channel, inout_type, amount)` or rely on monthly partitioning if analyst queries are frequent.
   - **Effect / Confidence:** Avoids full scans on analyst queries. **Low–Medium** (confirm once analyst SQL and query frequency are known).

#### 2. Things That Look Fine and Should Not Be Changed
- `shared/ledger/sql/lock_accounts.sql` ascending `account_id` lock order + `FOR UPDATE WAIT 1` (deadlock-free, bounded waits).
- `outbox_relay/sql/delete_delivered.sql` (array DML by explicit `event_id` PK) + `select_batch.sql` + `VARCHAR2(4000) IS JSON` (preserves delivery order without CLOB overhead).
- `0011_performance.sql` identity caches (`CACHE 10000` on `tran_id`/`transfer_id`, `CACHE 1000` on `attempt_id`/`customer_id`/`account_id`) and `01_saltation.sql` 3×1 GB redo log groups.
- `atm/sql/expiry_candidates.sql`, `atm/sql/open_session.sql`, `shared/ledger/sql/add_daily.sql` (`MERGE` under account lock), and `accounts/sql/select_balance.sql` (`MAX(business_date)` via `account_transaction_date_ix`).
