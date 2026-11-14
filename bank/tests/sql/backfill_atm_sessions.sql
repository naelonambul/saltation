-- test.backfill_atm_sessions: inspect business-clock session state without secrets.
-- Access path: ATM_SESSION full scan (test helper).
SELECT JSON_OBJECT('session_id' VALUE session_id,
 'started_at' VALUE TO_CHAR(started_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'expires_at' VALUE TO_CHAR(expires_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'ended_at' VALUE TO_CHAR(ended_at, 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
 'end_reason' VALUE end_reason)
FROM atm_session WHERE terminal_id = :terminal ORDER BY started_at, session_id
