-- test.product_counterparties: reads both legs and expected other account holder metadata.
-- Access path: ACCOUNT_TRANSACTION_DATE_IX then ACCOUNT_TRANSACTION_TRANSFER_IX and account/customer primary keys.
SELECT r.counterparty_bank_code, r.counterparty_account_no, r.counterparty_name, a.account_no, c.name
FROM account_transaction r JOIN account_transaction other ON other.transfer_id = r.transfer_id AND other.account_id <> r.account_id
JOIN account a ON a.account_id = other.account_id JOIN customer c ON c.customer_id = a.customer_id
WHERE r.transfer_id IN (SELECT transfer_id FROM account_transaction WHERE account_id = :id)
ORDER BY r.transfer_id, r.account_id
