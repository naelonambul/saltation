-- internal.insert_customer: a seeded customer.
-- Access path: CUSTOMER unique keys (number, CI hash, phone).
INSERT INTO customer (customer_no, ci_hash, name, phone, address, customer_type, created_at)
VALUES (:customer_no, :ci_hash, :name, :phone, :address, :customer_type, :now)
