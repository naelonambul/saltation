-- internal.insert_seed_request: the request record for a seeded opening balance.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, status, rsp_code, occurred_at)
VALUES (:request_id, 'SEED_OPENING', 'SYSTEM', 'GENERATOR', 'seed', :account_no, 'seed', 'DONE', 'A0000', :now)
