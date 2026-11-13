-- inbound.select_stored: read a committed result for replay after a uniqueness conflict.
-- Access path: TRANSACTION_REQUEST_IDEM_UK.
SELECT request_hash, status, rsp_code, response_json FROM transaction_request WHERE idem_scope = :scope AND idem_key = :key
