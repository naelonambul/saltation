-- test.rollover_rows: counts all job bookkeeping for one date.
-- Access path: ROLLOVER_RUN primary key.
SELECT COUNT(*) FROM rollover_run WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
