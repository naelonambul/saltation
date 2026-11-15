-- test.atm_requests: inspect stored refusals.
-- Access path: TRANSACTION_REQUEST primary key.
SELECT JSON_OBJECT('request_id' VALUE request_id, 'status' VALUE status, 'request_type' VALUE request_type,
 'channel' VALUE channel, 'terminal_id' VALUE terminal_id, 'rsp_code' VALUE rsp_code,
 'bank_rsp_code' VALUE bank_rsp_code, 'idem_scope' VALUE idem_scope, 'idem_key' VALUE idem_key,
 'ip' VALUE ip, 'region' VALUE region, 'device_id' VALUE device_id)
FROM transaction_request WHERE request_id = :request
