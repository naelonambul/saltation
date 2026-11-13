-- password_reset.peek_code: the newest code of the phone's customer, read without a lock so it can be verified outside the transaction.
-- Access path: CUSTOMER_PHONE_UK, then PASSWORD_RESET_CUSTOMER_IX for the newest row.
SELECT r.reset_id, r.code_hash FROM customer c JOIN password_reset r ON r.customer_id = c.customer_id
WHERE c.phone = :phone
  AND r.reset_id = (SELECT MAX(r2.reset_id) FROM password_reset r2 WHERE r2.customer_id = c.customer_id)
