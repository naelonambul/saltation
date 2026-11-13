-- transfers.registered: whether the sending customer has registered the recipient.
-- Access path: PAYEE_CUSTOMER_IX, filtered by active recipient.
SELECT COUNT(*) FROM payee WHERE customer_id = :customer_id AND bank_code = :bank_code
AND account_no = :account_no AND deleted_at IS NULL
