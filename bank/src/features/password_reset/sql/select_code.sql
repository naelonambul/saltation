-- password_reset.select_code: locks the customer's newest code; older codes are never considered again.
-- Access path: PASSWORD_RESET_CUSTOMER_IX for the newest row, then PASSWORD_RESET primary key.
SELECT reset_id, used_at, expires_at, failed_count FROM password_reset
WHERE reset_id = (SELECT /*+ NO_UNNEST INDEX(r2 password_reset_customer_ix) */ MAX(r2.reset_id) FROM password_reset r2 WHERE r2.customer_id = :customer_id) FOR UPDATE
