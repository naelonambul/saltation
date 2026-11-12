-- test.generator_transfer_requests: transfer requests from the generator listener since a time.
SELECT request_id FROM transaction_request
WHERE request_type = 'TRANSFER' AND listener = 'GENERATOR' AND recorded_at >= :since
