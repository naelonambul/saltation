-- auth.lock_credential: locks the credential and returns its current hash, so a login verified
-- against a password that was changed meanwhile issues no session.
-- Access path: CUSTOMER_CREDENTIAL primary key.
SELECT password_hash FROM customer_credential WHERE customer_id = :customer_id FOR UPDATE
