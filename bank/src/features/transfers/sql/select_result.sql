-- transfers.select_result: a customer's earlier transfer by bank_tran_id.
-- Access path: TRANSACTION_REQUEST_IDEM_UK.
SELECT status, rsp_code, response_json FROM transaction_request
WHERE idem_scope = :idem_scope AND idem_key = :idem_key AND request_type = 'TRANSFER'
