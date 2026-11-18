-- atm.select_lock_status: read the current PIN lock without acquiring a credential lock.
-- Access path: ATM_CREDENTIAL primary key.
SELECT locked_until FROM atm_credential WHERE account_no = :account
