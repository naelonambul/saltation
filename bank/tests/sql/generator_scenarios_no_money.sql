SELECT
    (SELECT COUNT(*) FROM transaction_request WHERE request_id = q.id OR reused_request_id = q.id)
    + (SELECT COUNT(*) FROM transfer WHERE request_id = q.id)
    + (SELECT COUNT(*) FROM account_transaction WHERE request_id = q.id)
FROM (SELECT :request_id AS id FROM dual) q
