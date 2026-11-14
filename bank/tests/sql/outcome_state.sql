SELECT value FROM (
SELECT 'password_reset:' || json_object(t.*) AS value FROM password_reset t
UNION ALL
SELECT 'sms_outbox:' || json_object(t.*) AS value FROM sms_outbox t
UNION ALL
SELECT 'payee:' || json_object(t.*) AS value FROM payee t
UNION ALL
SELECT 'deposit_detail:' || json_object(t.*) AS value FROM deposit_detail t
UNION ALL
SELECT 'loan_detail:' || json_object(t.*) AS value FROM loan_detail t
UNION ALL
SELECT 'transaction_request:' || json_object(t.*) AS value FROM transaction_request t
UNION ALL
SELECT 'transfer:' || json_object(t.*) AS value FROM transfer t
UNION ALL
SELECT 'customer_credential:' || json_object(t.*) AS value FROM customer_credential t
UNION ALL
SELECT 'account:' || json_object(t.*) AS value FROM account t
UNION ALL
SELECT 'login_attempt:' || json_object(t.*) AS value FROM login_attempt t
UNION ALL
SELECT 'token:' || json_object('hash' VALUE token_hash, 'revoked' VALUE revoked_at) AS value FROM access_token
UNION ALL
SELECT 'session:' || json_object('id' VALUE session_id, 'ended' VALUE ended_at, 'reason' VALUE end_reason) AS value FROM auth_session
) ORDER BY value
