-- Resolve ownership and the current login phone by CUSTOMER_NO_UK.
SELECT customer_id, phone FROM customer WHERE customer_no = :customer_no
