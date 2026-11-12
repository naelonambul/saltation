-- accounts.select_user: the customer's name.
-- Access path: CUSTOMER primary key.
SELECT name FROM customer WHERE customer_id = :customer_id
