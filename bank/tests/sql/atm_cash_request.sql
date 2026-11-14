-- test.atm_cash_request: includes conflict attempts with reused request IDs.
-- Access path: TRANSACTION_REQUEST primary key or reused ID.
SELECT JSON_OBJECT('status' VALUE status, 'request_type' VALUE request_type,
 'terminal_id' VALUE terminal_id, 'bank_tran_id' VALUE bank_tran_id,
 'idem_scope' VALUE idem_scope, 'idem_key' VALUE idem_key, 'bank_rsp_code' VALUE bank_rsp_code)
FROM transaction_request WHERE request_id = :id OR reused_request_id = :id
