-- inbound.insert_request: claim a client idempotency key before posting a deposit.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, bank_tran_id, status, rsp_code, device_id, ip, region, occurred_at) VALUES (:id, 'INBOUND', 'EXTERNAL', :listener, :scope, :key, :hash, :btid, 'DONE', 'A0000', :device, :ip, :region, :now)
