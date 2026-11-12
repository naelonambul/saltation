-- accounts.select_transactions_desc: one page of an account's rows, newest first.
-- Access path: ACCOUNT_TRANSACTION_SEQ_UK range scan descending on (account_id, account_sequence).
SELECT account_sequence, occurred_at, amount, balance_after, inout_type, tran_type, print_content
FROM account_transaction
WHERE account_id = :account_id
  AND occurred_at >= :from_time AND occurred_at < :to_time
  AND (:inquiry_type = 'A' OR (:inquiry_type_in = 'I' AND amount > 0) OR (:inquiry_type_out = 'O' AND amount < 0))
  AND account_sequence < :before_sequence
ORDER BY account_sequence DESC
FETCH FIRST :page_size ROWS ONLY
