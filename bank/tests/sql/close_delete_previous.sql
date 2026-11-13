-- test.close_delete_previous: daily close integration evidence.
-- Access path: test schema indexes and scans.
DELETE FROM daily_close WHERE account_id = (SELECT account_id FROM account WHERE account_no = :account_no) AND business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
