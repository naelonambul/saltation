SELECT transfer_id, kind, amount
FROM transfer
WHERE request_id = :request_id
