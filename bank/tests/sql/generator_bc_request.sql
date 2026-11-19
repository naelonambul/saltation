-- Include any attempt recorded against the original request ID.
SELECT request_id, request_type, status
FROM transaction_request
WHERE request_id = :request_id OR reused_request_id = :request_id
