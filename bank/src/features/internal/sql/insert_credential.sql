-- internal.insert_credential: the seeded customer's password hash.
-- Access path: CUSTOMER_CREDENTIAL primary key.
INSERT INTO customer_credential (customer_id, password_hash, password_changed_at)
VALUES (:customer_id, :password_hash, :now)
