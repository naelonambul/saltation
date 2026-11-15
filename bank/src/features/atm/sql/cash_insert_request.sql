-- atm.cash_insert_request: claim the terminal-scoped key before checking business rules.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (
 request_id, request_type, channel, listener, idem_scope, idem_key, request_hash,
 bank_tran_id, status, rsp_code, terminal_id, session_id, device_id, ip, region, occurred_at)
VALUES (:id, :kind, 'ATM', :listener, :scope, :key, :hash,
 :bank_tran_id, 'DONE', 'A0000', :terminal, :session_id, :device, :ip, :region, :at)
