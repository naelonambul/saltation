-- test.atm_close_account: close a test account.
-- Access path: ACCOUNT unique key.
UPDATE account SET status = 'CLOSED' WHERE account_no = :account
