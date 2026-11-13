-- password_reset.select_customer: locks the customer owning a phone to serialize reset requests and confirmations.
-- Access path: CUSTOMER_PHONE_UK.
SELECT customer_id, customer_no FROM customer WHERE phone = :phone FOR UPDATE
