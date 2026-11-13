-- rollover.finish: finishes a job only once in the event transaction.
-- Access path: ROLLOVER_RUN primary key.
UPDATE rollover_run SET finished_at = SYSTIMESTAMP WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD') AND job = :job AND finished_at IS NULL
