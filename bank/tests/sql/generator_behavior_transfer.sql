SELECT JSON_OBJECT(
    'request_id' VALUE r.request_id,
    'request_type' VALUE r.request_type,
    'status' VALUE r.status,
    'channel' VALUE r.channel,
    'listener' VALUE r.listener,
    'rsp_code' VALUE r.rsp_code,
    'bank_rsp_code' VALUE r.bank_rsp_code,
    'session_id' VALUE r.session_id,
    'device_id' VALUE r.device_id,
    'ip' VALUE r.ip,
    'region' VALUE r.region,
    'occurred_at' VALUE TO_CHAR(r.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'transfer_id' VALUE t.transfer_id,
    'kind' VALUE t.kind,
    'amount' VALUE t.amount,
    'transfer_at' VALUE TO_CHAR(t.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
)
FROM transaction_request r
LEFT JOIN transfer t ON t.request_id = r.request_id
WHERE r.request_id = :request_id OR r.reused_request_id = :request_id
