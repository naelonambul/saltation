-- test.rollover_finished: counts finished job rows for the sequence.
-- Access path: ROLLOVER_RUN full scan on the fresh test schema.
SELECT COUNT(*) FROM rollover_run WHERE finished_at IS NOT NULL AND business_date >= TO_DATE(:business_date, 'YYYY-MM-DD') AND job IN ('first', 'second')
