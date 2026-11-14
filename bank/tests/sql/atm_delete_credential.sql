-- test.atm_delete_credential: make an account PIN-less.
-- Access path: ATM_CREDENTIAL primary key.
DELETE FROM atm_credential WHERE account_no = :account
