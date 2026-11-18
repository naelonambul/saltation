-- cards.finish_payment: record the final outcome and replay data.
-- Access path: TRANSACTION_REQUEST primary key.
UPDATE transaction_request SET status = :new_status, rsp_code = :code, bank_rsp_code = :bank_code,
  response_json = :response WHERE request_id = :id
