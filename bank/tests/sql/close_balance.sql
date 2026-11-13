-- test.close_balance: daily close integration evidence.
-- Access path: test schema indexes and scans.
UPDATE account SET balance = balance + :delta WHERE account_no = :account_no
