-- admin.view_transactions: page the account ledger by KST business day and sequence.
WITH params AS (
  SELECT :account_id account_id,
         CAST(:from_date AS DATE) from_date,
         CAST(:before_date AS DATE) before_date,
         :channel filter_channel, :direction filter_direction,
         :min_amount min_amount, :max_amount max_amount,
         CAST(:cursor_date AS DATE) cursor_date, :cursor_sequence cursor_sequence
  FROM dual
)
SELECT TO_CHAR(tx.business_date, 'YYYY-MM-DD') cursor_date,
       TO_CHAR(tx.account_sequence, 'TM9') cursor_sequence,
       JSON_OBJECT(
         'tran_id' VALUE TO_CHAR(tx.tran_id, 'TM9'),
         'account_no' VALUE a.account_no,
         'account_sequence' VALUE TO_CHAR(tx.account_sequence, 'TM9'),
         'amount' VALUE TO_CHAR(tx.amount, 'TM9'),
         'balance_after' VALUE TO_CHAR(tx.balance_after, 'TM9'),
         'business_date' VALUE TO_CHAR(tx.business_date, 'YYYY-MM-DD'),
         'occurred_at' VALUE TO_CHAR(tx.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
         'channel' VALUE tx.channel,
         'transfer_id' VALUE TO_CHAR(tx.transfer_id, 'TM9'),
         'request_id' VALUE tx.request_id,
         'inout_type' VALUE tx.inout_type,
         'tran_type' VALUE tx.tran_type,
         'print_content' VALUE tx.print_content,
         'counterparty_bank_code' VALUE tx.counterparty_bank_code,
         'counterparty_account_no' VALUE tx.counterparty_account_no,
         'counterparty_name' VALUE tx.counterparty_name,
         'merchant_name' VALUE ca.merchant_name,
         'mcc' VALUE ca.mcc
       ) item
FROM params p
JOIN account a ON a.account_id = p.account_id
JOIN account_transaction tx ON tx.account_id = a.account_id
LEFT JOIN card_authorization ca ON ca.request_id = tx.request_id
WHERE (p.from_date IS NULL OR tx.business_date >= p.from_date)
  AND (p.before_date IS NULL OR tx.business_date < p.before_date)
  AND (p.filter_channel IS NULL OR tx.channel = p.filter_channel)
  AND (p.filter_direction IS NULL
       OR (p.filter_direction = 'debit' AND tx.amount < 0)
       OR (p.filter_direction = 'credit' AND tx.amount > 0))
  AND (p.min_amount IS NULL OR ABS(tx.amount) >= p.min_amount)
  AND (p.max_amount IS NULL OR ABS(tx.amount) <= p.max_amount)
  AND (p.cursor_date IS NULL OR (
    tx.business_date <= p.cursor_date
    AND (tx.business_date < p.cursor_date
         OR (tx.business_date = p.cursor_date AND tx.account_sequence < p.cursor_sequence))
  ))
ORDER BY tx.business_date DESC, tx.account_sequence DESC
FETCH FIRST :take_count ROWS ONLY
