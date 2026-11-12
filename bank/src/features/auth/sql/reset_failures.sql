-- auth.reset_failures: clears the failure count and lock.
-- Access path: CUSTOMER_CREDENTIAL primary key.
UPDATE customer_credential SET failed_count = 0, locked_until = NULL WHERE customer_id = :customer_id
