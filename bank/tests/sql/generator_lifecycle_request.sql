SELECT JSON_OBJECT(
    'id' VALUE q.request_id,
    'type' VALUE q.request_type,
    'listener' VALUE q.listener,
    'status' VALUE q.status,
    'bank_code' VALUE q.bank_rsp_code,
    'posting_id' VALUE t.transfer_id,
    'kind' VALUE t.kind,
    'amount' VALUE t.amount,
    'account' VALUE a.account_no,
    'leg' VALUE l.amount,
    'date' VALUE TO_CHAR(l.business_date, 'YYYY-MM-DD'),
    'at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'recorded_at' VALUE TO_CHAR(l.recorded_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'counterparty' VALUE l.counterparty_account_no
)
FROM transaction_request q
LEFT JOIN transfer t ON t.request_id = q.request_id
LEFT JOIN account_transaction l ON l.transfer_id = t.transfer_id AND l.request_id = q.request_id
LEFT JOIN account a ON a.account_id = l.account_id
WHERE q.request_id = :id OR q.reused_request_id = :id
