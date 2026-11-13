UPDATE transaction_request SET request_id = :new_id, reused_request_id = :original_id
WHERE request_id = :request_id AND idem_key IS NULL AND status = 'REJECTED'
