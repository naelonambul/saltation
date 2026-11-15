-- atm.pin_hash: read the PIN hash before verification.
-- Access path: ATM_CREDENTIAL primary key.
SELECT pin_hash FROM atm_credential WHERE account_no = :account
