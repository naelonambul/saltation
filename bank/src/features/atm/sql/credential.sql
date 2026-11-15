-- atm.credential: reread credential after acquiring the lock.
-- Access path: ATM_CREDENTIAL and ACCOUNT unique keys.
SELECT a.status, c.failed_count, c.locked_until FROM atm_credential c JOIN account a ON a.account_no = c.account_no WHERE c.account_no = :account
