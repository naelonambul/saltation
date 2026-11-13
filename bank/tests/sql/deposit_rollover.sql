-- test.deposit_rollover: proves maturity finished before dormancy started for its business date.
-- Access path: ROLLOVER_RUN primary key for both jobs.
SELECT COUNT(*) FROM rollover_run m JOIN rollover_run d ON d.business_date = m.business_date
WHERE m.business_date = TO_DATE(:date_text, 'YYYYMMDD') AND m.job = 'maturity' AND d.job = 'dormancy'
AND m.finished_at IS NOT NULL AND d.finished_at IS NOT NULL AND m.finished_at <= d.started_at
