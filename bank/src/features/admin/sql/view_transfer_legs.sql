-- admin.view_transfer_legs: the bounded set of rows posted for one transfer.
SELECT JSON_OBJECT(
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
         'counterparty_name' VALUE tx.counterparty_name
       ) item
FROM account_transaction tx
JOIN account a ON a.account_id = tx.account_id
WHERE tx.transfer_id = :transfer_id
ORDER BY tx.tran_id
