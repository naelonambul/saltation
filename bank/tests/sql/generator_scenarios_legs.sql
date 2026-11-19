SELECT JSON_OBJECT(
    'account' VALUE a.account_no,
    'request_id' VALUE l.request_id,
    'transfer_id' VALUE l.transfer_id,
    'amount' VALUE l.amount,
    'balance_after' VALUE l.balance_after,
    'channel' VALUE l.channel,
    'date' VALUE TO_CHAR(l.business_date, 'YYYY-MM-DD'),
    'occurred_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'counterparty_bank' VALUE l.counterparty_bank_code,
    'counterparty_account' VALUE l.counterparty_account_no
)
FROM account_transaction l
JOIN account a ON a.account_id = l.account_id
WHERE a.account_no = :account_no
ORDER BY l.account_sequence
