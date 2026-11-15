-- atm.lock_credential: serialize account PIN attempts.
-- Access path: ATM_CREDENTIAL primary key.
SELECT account_no FROM atm_credential WHERE account_no = :account FOR UPDATE
