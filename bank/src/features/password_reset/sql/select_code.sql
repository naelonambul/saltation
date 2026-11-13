-- password_reset.select_code: locks the customer's newest code; older codes are never considered again.
-- Access path: PASSWORD_RESET_CUSTOMER_IX for the newest row, then PASSWORD_RESET primary key.
SELECT reset_id, used_at, expires_at, failed_count FROM password_reset
WHERE reset_id = (SELECT MAX(reset_id) FROM password_reset WHERE customer_id = :customer_id) FOR UPDATE
