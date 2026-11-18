-- 0019_tune_daily_close: date-leading compressed key serves date reconciliation.
-- Original DDL:
-- ALTER TABLE daily_close ADD CONSTRAINT daily_close_pk PRIMARY KEY (account_id, business_date)
-- CREATE INDEX daily_close_date_ix ON daily_close (business_date, verified)
-- Restore only through a forward migration with those original definitions.
CREATE UNIQUE INDEX daily_close_tune_pk_ix ON daily_close (business_date, account_id) COMPRESS 1 ONLINE
/
ALTER TABLE daily_close DROP PRIMARY KEY DROP INDEX
/
ALTER INDEX daily_close_tune_pk_ix RENAME TO daily_close_pk
/
ALTER TABLE daily_close ADD CONSTRAINT daily_close_pk PRIMARY KEY (business_date, account_id) USING INDEX daily_close_pk
/
DROP INDEX daily_close_date_ix
