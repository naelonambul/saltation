-- atm.lock_credential: serialize account PIN attempts without locking the account.
-- Access path: ATM_CREDENTIAL and ACCOUNT unique keys.
SELECT a.status, c.failed_count, c.locked_until FROM atm_credential c JOIN account a ON a.account_no = c.account_no WHERE c.account_no = :account FOR UPDATE OF c.account_no WAIT 1
