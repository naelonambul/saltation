-- test.accounts_product_history: stored product account ledger rows in sequence order.
-- Access path: FINTECH_ACCOUNT primary key, then ACCOUNT_TRANSACTION_DATE_IX by account.
SELECT tx.amount, tx.balance_after, tx.inout_type
FROM fintech_account f JOIN account_transaction tx ON tx.account_id = f.account_id
WHERE f.fintech_use_num = :fintech_use_num
ORDER BY tx.account_sequence
