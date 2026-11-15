-- test.e2e_atm_session: resolve a smoke session by its start request, without credentials.
-- Access path: ATM_SESSION full scan (bounded integration fixture; request_id has no index).
SELECT JSON_OBJECT('session_id' VALUE session_id, 'account_no' VALUE account_no,
 'terminal_id' VALUE terminal_id, 'end_reason' VALUE end_reason,
 'ended' VALUE CASE WHEN ended_at IS NULL THEN 0 ELSE 1 END)
FROM atm_session WHERE request_id = :request_id
