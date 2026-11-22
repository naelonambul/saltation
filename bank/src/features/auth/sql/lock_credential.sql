-- auth.lock_credential: locks the credential and returns its current hash,
-- failure count, and lock expiry so concurrent logins serialize lock state
-- transitions and never issue a session against a locked or changed password.
-- Access path: CUSTOMER_CREDENTIAL primary key.
SELECT password_hash, failed_count, locked_until FROM customer_credential WHERE customer_id = :customer_id FOR UPDATE
