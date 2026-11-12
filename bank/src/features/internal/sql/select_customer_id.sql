-- internal.select_customer_id: customer ID by customer number.
-- Access path: CUSTOMER_NO_UK.
SELECT customer_id FROM customer WHERE customer_no = :customer_no
