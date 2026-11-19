-- test.admin_test_close: prepare one stored result for the read-only admin query.
INSERT INTO daily_close (account_id, business_date, opening_balance, closing_balance, verified, mismatch_detail)
SELECT account_id, DATE '2001-11-19', 10, 20, :verified, :detail
FROM account WHERE account_no = :account_no
