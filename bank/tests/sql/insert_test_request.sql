-- test.insert_test_request: a request row that test transaction rows can reference.
INSERT INTO transaction_request (request_id, request_type, channel, listener, idem_scope, idem_key, request_hash, status, rsp_code, occurred_at)
VALUES (:request_id, 'TEST', 'MOBILE', 'GENERATOR', 'test', :request_id2, 'x', 'DONE', 'A0000', :now)
