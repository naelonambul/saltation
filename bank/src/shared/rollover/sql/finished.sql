-- rollover.finished: counts completed jobs for this date.
-- Access path: ROLLOVER_RUN primary key.
SELECT COUNT(*) FROM rollover_run WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD') AND job = :job AND finished_at IS NOT NULL
