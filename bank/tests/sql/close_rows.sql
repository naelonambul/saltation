-- test.close_rows: daily close integration evidence.
-- Access path: test schema indexes and scans.
SELECT a.account_no, c.opening_balance, c.closing_balance, c.verified, c.mismatch_detail
 FROM daily_close c JOIN account a ON a.account_id = c.account_id
 WHERE c.business_date = TO_DATE(:business_date, 'YYYY-MM-DD') ORDER BY a.account_no
