-- test.atm_sessions: inspect sessions at a terminal.
-- Access path: ATM_SESSION terminal index.
SELECT JSON_OBJECT('session_id' VALUE session_id, 'account_no' VALUE account_no,
 'token_hash' VALUE token_hash, 'started_at' VALUE TO_CHAR(started_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'expires_at' VALUE TO_CHAR(expires_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'ended_at' VALUE TO_CHAR(ended_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'end_reason' VALUE end_reason, 'request_id' VALUE request_id)
FROM atm_session WHERE terminal_id = :terminal ORDER BY started_at, session_id
