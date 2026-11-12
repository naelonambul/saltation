-- transfers.insert_request: claims the idempotency key first. A concurrent
-- duplicate blocks on TRANSACTION_REQUEST_IDEM_UK until this transaction ends,
-- then fails with ORA-00001 and replays the stored result.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (
  request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, bank_tran_id,
  status, rsp_code, session_id, device_id, ip, region, occurred_at)
VALUES (
  :request_id, 'TRANSFER', 'MOBILE', :listener, :idem_scope, :idem_key, :request_hash, :bank_tran_id,
  'DONE', 'A0000', :session_id, :device_id, :ip, :region, :occurred_at)
