-- accounts.select_transactions_desc: sequence-ordered page within the owned account's time bounds.
-- Access path: ACCOUNT_TRANSACTION_SEQ_UK stopkey; boundary sequences come from select_history_account.
SELECT /*+ INDEX_DESC(tx account_transaction_seq_uk) */
       tx.account_sequence, tx.occurred_at, tx.amount, tx.balance_after,
       tx.inout_type, tx.tran_type, tx.print_content
FROM account_transaction tx
WHERE tx.account_id = :account_id
  AND tx.occurred_at >= :from_time AND tx.occurred_at < :to_time
  AND (:inquiry_type = 'A' OR (:inquiry_type_in = 'I' AND tx.amount > 0) OR (:inquiry_type_out = 'O' AND tx.amount < 0))
  AND tx.account_sequence < :before_sequence
  AND tx.account_sequence >= :first_sequence
ORDER BY tx.account_sequence DESC
FETCH FIRST :page_size ROWS ONLY
