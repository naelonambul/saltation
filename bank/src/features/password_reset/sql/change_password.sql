-- password_reset.change_password: changes the password and clears the login lock and failures.
-- Access path: CUSTOMER_CREDENTIAL primary key.
UPDATE customer_credential SET password_hash = :hash, password_changed_at = :now, failed_count = 0, locked_until = NULL WHERE customer_id = :customer_id
