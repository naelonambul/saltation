-- auth.select_login: the customer and credential for a phone number.
-- Access path: CUSTOMER_PHONE_UK, then CUSTOMER_CREDENTIAL primary key.
SELECT c.customer_id, c.customer_no, cr.password_hash, cr.failed_count, cr.locked_until
FROM customer c JOIN customer_credential cr ON cr.customer_id = c.customer_id
WHERE c.phone = :phone
