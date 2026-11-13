-- test.close_final: daily close integration evidence.
-- Access path: test schema indexes and scans.
WITH dates AS (
 SELECT first_date + LEVEL - 1 d FROM
 (SELECT MIN(business_date) first_date FROM rollover_run),
 sim_clock s
 CONNECT BY LEVEL <= s.rolled_through - first_date + 1
)
SELECT (SELECT COUNT(*) FROM daily_close WHERE verified <> 'Y') +
 (SELECT COUNT(*) FROM dates d WHERE NOT EXISTS (
 SELECT 1 FROM rollover_run r WHERE r.business_date = d.d AND r.job = 'daily_close' AND r.finished_at IS NOT NULL))
 FROM dual
