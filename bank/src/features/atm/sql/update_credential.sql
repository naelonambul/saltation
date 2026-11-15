-- atm.update_credential: persist failure count and lock.
-- Access path: ATM_CREDENTIAL primary key.
UPDATE atm_credential SET failed_count = :failed, locked_until = :locked WHERE account_no = :account
