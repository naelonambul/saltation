-- auth.select_failures: the failure count and lock after an update in this transaction.
-- Access path: CUSTOMER_CREDENTIAL primary key.
SELECT failed_count, locked_until FROM customer_credential WHERE customer_id = :customer_id
