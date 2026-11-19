SELECT JSON_OBJECT(
    'request_id' VALUE q.request_id,
    'request_type' VALUE q.request_type,
    'listener' VALUE q.listener,
    'status' VALUE q.status,
    'rsp_code' VALUE q.rsp_code,
    'bank_rsp_code' VALUE q.bank_rsp_code,
    'channel' VALUE q.channel,
    'idempotency_key' VALUE q.idem_key,
    'session_id' VALUE q.session_id,
    'terminal_id' VALUE q.terminal_id,
    'occurred_at' VALUE TO_CHAR(q.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'transfer_count' VALUE (SELECT COUNT(*) FROM transfer f WHERE f.request_id = q.request_id),
    'leg_count' VALUE (SELECT COUNT(*) FROM account_transaction l WHERE l.request_id = q.request_id),
    'transfer_id' VALUE t.transfer_id,
    'kind' VALUE t.kind,
    'amount' VALUE t.amount,
    'transfer_at' VALUE TO_CHAR(t.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'leg_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'business_date' VALUE TO_CHAR(l.business_date, 'YYYY-MM-DD'),
    'account_no' VALUE a.account_no,
    'leg_amount' VALUE l.amount,
    'leg_channel' VALUE l.channel,
    'leg_terminal_id' VALUE l.terminal_id,
    'inout_type' VALUE l.inout_type,
    'counterparty_name' VALUE l.counterparty_name,
    'counterparty_bank_code' VALUE l.counterparty_bank_code,
    'counterparty_account_no' VALUE l.counterparty_account_no,
    'balance_after' VALUE l.balance_after
)
FROM transaction_request q
LEFT JOIN transfer t ON t.request_id = q.request_id
LEFT JOIN account_transaction l ON l.transfer_id = t.transfer_id AND l.request_id = q.request_id
LEFT JOIN account a ON a.account_id = l.account_id
WHERE q.request_id = :id OR q.reused_request_id = :id
