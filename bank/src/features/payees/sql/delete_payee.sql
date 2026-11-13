-- payees.delete_payee: marks the customer's active payee deleted.
-- Access path: PAYEE primary key.
UPDATE payee SET deleted_at = :deleted_at WHERE payee_id = :payee_id AND customer_id = :customer_id AND deleted_at IS NULL
