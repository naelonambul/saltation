-- test.close_restore_previous: daily close integration evidence.
-- Access path: test schema indexes and scans.
INSERT INTO daily_close (account_id, business_date, opening_balance, closing_balance, verified, mismatch_detail) SELECT account_id, TO_DATE(:business_date, 'YYYY-MM-DD'), :opening_balance, :closing_balance, 'Y', NULL FROM account WHERE account_no = :account_no
