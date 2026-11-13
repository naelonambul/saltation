-- rollover.start: starts or restarts an unfinished job.
-- Access path: ROLLOVER_RUN primary key.
MERGE INTO rollover_run r
USING (SELECT TO_DATE(:business_date, 'YYYY-MM-DD') AS business_date, :job AS job FROM dual) s
ON (r.business_date = s.business_date AND r.job = s.job)
WHEN MATCHED THEN UPDATE SET started_at = SYSTIMESTAMP WHERE r.finished_at IS NULL
WHEN NOT MATCHED THEN INSERT (business_date, job, started_at) VALUES (s.business_date, s.job, SYSTIMESTAMP)
