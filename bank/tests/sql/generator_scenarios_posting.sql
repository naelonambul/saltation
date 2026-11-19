SELECT r.request_id, r.request_type, r.status, r.rsp_code, r.session_id,
       t.transfer_id, t.kind, t.amount, t.payee_registered,
       TO_CHAR(r.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
       TO_CHAR(t.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM transaction_request r
LEFT JOIN transfer t ON t.request_id = r.request_id
WHERE r.request_id = :request_id OR r.reused_request_id = :request_id
