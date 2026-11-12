-- transfers.finish_request: the request's final outcome and replayable response.
-- Access path: TRANSACTION_REQUEST primary key.
UPDATE transaction_request
SET status = :status, rsp_code = :rsp_code, bank_rsp_code = :bank_rsp_code, response_json = :response_json
WHERE request_id = :request_id
