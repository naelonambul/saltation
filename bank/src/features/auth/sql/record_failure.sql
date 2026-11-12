-- auth.record_failure: counts a failed login; the fifth consecutive failure locks.
-- Access path: CUSTOMER_CREDENTIAL primary key.
UPDATE customer_credential
SET failed_count = failed_count + 1,
    locked_until = CASE WHEN failed_count + 1 >= 5 THEN :lock_until ELSE locked_until END
WHERE customer_id = :customer_id
