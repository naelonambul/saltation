-- atm.cash_finish: store the replayable result without releasing the key.
-- Access path: TRANSACTION_REQUEST primary key.
UPDATE transaction_request SET status = :status, rsp_code = :code,
 bank_rsp_code = :bank, response_json = :response WHERE request_id = :id
