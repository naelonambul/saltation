-- admin.view_transfer: expose the original request outcome with an explicit field allowlist.
SELECT JSON_OBJECT(
         'transfer_id' VALUE TO_CHAR(t.transfer_id, 'TM9'),
         'kind' VALUE t.kind,
         'amount' VALUE TO_CHAR(t.amount, 'TM9'),
         'occurred_at' VALUE TO_CHAR(t.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
         'request' VALUE CASE WHEN r.request_id IS NOT NULL THEN JSON_OBJECT(
           'request_id' VALUE r.request_id,
           'request_type' VALUE r.request_type,
           'channel' VALUE r.channel,
           'status' VALUE r.status,
           'rsp_code' VALUE r.rsp_code,
           'bank_rsp_code' VALUE r.bank_rsp_code,
           'occurred_at' VALUE TO_CHAR(r.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
         ) END FORMAT JSON
       ) item
FROM transfer t
LEFT JOIN transaction_request r ON r.request_id = t.request_id
WHERE t.transfer_id = :transfer_id
