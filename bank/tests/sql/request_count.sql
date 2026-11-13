-- test.request_count: verify recipient persistence.
-- Access path: TRANSACTION_REQUEST primary key.
SELECT COUNT(*) FROM transaction_request WHERE request_id = :request_id
