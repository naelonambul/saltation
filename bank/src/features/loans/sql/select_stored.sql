-- loans.select_stored: the stored outcome for an idempotency key.
-- Access path: TRANSACTION_REQUEST_IDEM_UK.
SELECT request_hash, status, rsp_code, response_json FROM transaction_request
WHERE idem_scope = :idem_scope AND idem_key = :idem_key
