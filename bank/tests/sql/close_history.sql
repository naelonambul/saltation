-- test.close_history: daily close integration evidence.
-- Access path: test schema indexes and scans.
SELECT
 (SELECT COUNT(*) FROM daily_close WHERE verified <> 'Y') +
 (SELECT COUNT(*) FROM rollover_run r WHERE r.job = 'first' AND r.finished_at IS NOT NULL AND NOT EXISTS
 (SELECT 1 FROM rollover_run c WHERE c.business_date = r.business_date AND c.job = 'daily_close' AND c.finished_at IS NOT NULL)) +
 (SELECT COUNT(*) FROM daily_close c WHERE c.closing_balance - c.opening_balance <>
 NVL((SELECT SUM(r.amount) FROM account_transaction r WHERE r.account_id = c.account_id AND r.occurred_at >= FROM_TZ(CAST(c.business_date AS TIMESTAMP), 'Asia/Seoul') AND r.occurred_at < FROM_TZ(CAST(c.business_date + 1 AS TIMESTAMP), 'Asia/Seoul')), 0)) +
 (SELECT COUNT(*) FROM daily_close c JOIN account_status_history h ON h.account_id = c.account_id
 WHERE h.to_status = 'CLOSED' AND h.occurred_at < FROM_TZ(CAST(c.business_date AS TIMESTAMP), 'Asia/Seoul')) +
 (SELECT COUNT(*) FROM account a CROSS JOIN rollover_run d WHERE d.job = 'daily_close' AND d.finished_at IS NOT NULL
 AND a.opened_at < FROM_TZ(CAST(d.business_date + 1 AS TIMESTAMP), 'Asia/Seoul')
 AND NOT EXISTS (SELECT 1 FROM account_status_history h WHERE h.account_id = a.account_id AND h.to_status = 'CLOSED' AND h.occurred_at < FROM_TZ(CAST(d.business_date AS TIMESTAMP), 'Asia/Seoul'))
 AND NOT EXISTS (SELECT 1 FROM daily_close c WHERE c.account_id = a.account_id AND c.business_date = d.business_date))
 FROM dual
