-- loans.insert_request: claims the customer idempotency key before any movement.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (
request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, bank_tran_id,
status, rsp_code, session_id, device_id, ip, region, occurred_at)
VALUES (:request_id, :request_type, 'MOBILE', :listener, :scope, :key, :hash, :bank_tran_id,
'DONE', 'A0000', :session_id, :device_id, :ip, :region, :at)
