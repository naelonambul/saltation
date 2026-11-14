-- outcome.insert_rejected: an unclaimed rejection, preserving a reused client request ID.
-- Access path: TRANSACTION_REQUEST primary key.
INSERT INTO transaction_request (
 request_id, reused_request_id, request_type, channel, listener,
 idem_scope, idem_key, request_hash, status, rsp_code, bank_rsp_code,
 bank_tran_id, session_id, device_id, ip, region, occurred_at)
VALUES (:id, :reused, :kind, :channel, :listener,
 NULL, NULL, NULL, 'REJECTED', :code, :bank_code,
 :bank_id, :session_id, :device, :ip, :region, :at)
