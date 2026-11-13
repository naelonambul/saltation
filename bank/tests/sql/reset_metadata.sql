-- test.reset_metadata: examines stored reset and credential hashes for plaintext secrets.
-- Access path: CUSTOMER_NO_UK joined to CUSTOMER_CREDENTIAL primary key and PASSWORD_RESET_CUSTOMER_IX.
SELECT cr.password_hash FROM customer c JOIN customer_credential cr ON cr.customer_id = c.customer_id WHERE c.customer_no = :customer_no
UNION ALL
SELECT r.code_hash FROM customer c JOIN password_reset r ON r.customer_id = c.customer_id WHERE c.customer_no = :customer_no2
