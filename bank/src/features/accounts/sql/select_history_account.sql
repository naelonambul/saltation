-- accounts.select_history_account: owned account and index-only sequence bounds for its time window.
-- Access path: FINTECH_ACCOUNT and ACCOUNT primary keys, ACCOUNT_TRANSACTION_TIME_IX covering range.
SELECT /*+ LEADING(f a tx) USE_NL(a tx) INDEX(tx account_transaction_time_ix) */
       a.account_id, a.account_no, a.balance,
       MIN(tx.account_sequence), MAX(tx.account_sequence)
FROM fintech_account f JOIN account a ON a.account_id = f.account_id
LEFT JOIN account_transaction tx ON tx.account_id = a.account_id
  AND tx.occurred_at >= :from_time AND tx.occurred_at < :to_time
WHERE f.fintech_use_num = :fintech_use_num AND a.customer_id = :customer_id
GROUP BY a.account_id, a.account_no, a.balance
