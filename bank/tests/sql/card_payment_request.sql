-- test.card_payment_request: request outcome and payment side effects for one HTTP request.
SELECT JSON_OBJECT(
 'request_id' VALUE q.request_id, 'reused_id' VALUE q.reused_request_id,
 'kind' VALUE q.request_type, 'channel' VALUE q.channel,
 'status' VALUE q.status, 'code' VALUE q.rsp_code, 'bank_code' VALUE q.bank_rsp_code,
 'key' VALUE q.idem_key, 'bank_id' VALUE q.bank_tran_id, 'session_id' VALUE q.session_id,
 'authorization' VALUE (SELECT COUNT(*) FROM card_authorization a WHERE a.request_id = q.request_id),
 'transfers' VALUE (SELECT COUNT(*) FROM transfer t WHERE t.request_id = q.request_id),
 'legs' VALUE (SELECT COUNT(*) FROM account_transaction t WHERE t.request_id = q.request_id)
)
FROM transaction_request q WHERE q.request_id = :id OR q.reused_request_id = :id
ORDER BY q.reused_request_id NULLS FIRST, q.request_id
