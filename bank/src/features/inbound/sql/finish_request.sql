-- inbound.finish_request: store the final deposit result for replay.
-- Access path: TRANSACTION_REQUEST primary key.
UPDATE transaction_request SET status = :status, rsp_code = :code, bank_rsp_code = :bank_code, response_json = :response WHERE request_id = :id
