-- test.request_status: the stored outcome of a request.
SELECT status || '/' || NVL(bank_rsp_code, '-') FROM transaction_request WHERE request_id = :request_id
