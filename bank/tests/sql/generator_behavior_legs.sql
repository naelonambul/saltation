SELECT JSON_OBJECT(
    'account_no' VALUE a.account_no,
    'request_id' VALUE l.request_id,
    'transfer_id' VALUE l.transfer_id,
    'amount' VALUE l.amount,
    'balance_after' VALUE l.balance_after,
    'inout_type' VALUE l.inout_type,
    'channel' VALUE l.channel,
    'counterparty_account_no' VALUE l.counterparty_account_no,
    'counterparty_bank_code' VALUE l.counterparty_bank_code,
    'business_date' VALUE TO_CHAR(l.business_date, 'YYYY-MM-DD'),
    'occurred_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
)
FROM account_transaction l
JOIN account a ON a.account_id = l.account_id
WHERE l.request_id = :request_id OR l.transfer_id = :transfer_id
ORDER BY l.amount
