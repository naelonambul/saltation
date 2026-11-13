-- password_reset.insert_code: stores a hashed reset code.
-- Access path: PASSWORD_RESET identity primary key.
INSERT INTO password_reset (customer_id, code_hash, issued_at, expires_at) VALUES (:customer_id, :code_hash, :issued_at, :expires_at)
