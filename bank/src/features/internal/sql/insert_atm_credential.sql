-- internal.insert_atm_credential: the demand account's ATM PIN hash.
-- Access path: ATM_CREDENTIAL primary key.
INSERT INTO atm_credential (account_no, pin_hash, failed_count, locked_until)
VALUES (:account_no, :pin_hash, 0, NULL)
