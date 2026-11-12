-- ledger.select_transfer_id: the transfer just inserted for a request.
-- Access path: TRANSFER_REQUEST_UK.
SELECT transfer_id FROM transfer WHERE request_id = :request_id
