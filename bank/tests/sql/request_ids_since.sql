-- test.request_ids_since: transfer requests with a given idempotency scope.
SELECT request_id FROM transaction_request WHERE idem_scope = :idem_scope AND request_type = 'TRANSFER'
