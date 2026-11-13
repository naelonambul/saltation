-- test.insert_request_attempt: exercise request idempotency constraints.
INSERT INTO transaction_request (request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, status, rsp_code, occurred_at)
VALUES (:request_id, 'TEST', 'MOBILE', 'GENERATOR', :idem_scope, :idem_key, :request_hash, :status, 'A0000', SYSTIMESTAMP)
