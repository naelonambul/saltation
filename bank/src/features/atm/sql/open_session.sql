-- atm.open_session: find the serialized terminal open session.
-- Access path: ATM_SESSION_TERMINAL_OPEN_UK (the predicate repeats the index expression).
SELECT session_id, account_no, expires_at, ended_at, request_id, terminal_id FROM atm_session WHERE CASE WHEN ended_at IS NULL THEN terminal_id END = :terminal
