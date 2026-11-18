-- transfers.registered: whether the sending customer has registered the recipient.
-- Access path: PAYEE_ACTIVE_UK, with the same expressions as the active unique key.
SELECT COUNT(*) FROM payee
WHERE CASE WHEN deleted_at IS NULL THEN customer_id END = :customer_id
  AND CASE WHEN deleted_at IS NULL THEN bank_code END = :bank_code
  AND CASE WHEN deleted_at IS NULL THEN account_no END = :account_no
