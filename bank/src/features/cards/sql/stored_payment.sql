-- cards.stored_payment: read the committed decision after a uniqueness conflict.
-- Access path: TRANSACTION_REQUEST_IDEM_UK.
SELECT request_hash, response_json FROM transaction_request WHERE idem_scope = :scope AND idem_key = :idem_key_value
