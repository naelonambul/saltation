-- auth.end_session: ends a session on logout.
-- Access path: AUTH_SESSION primary key.
UPDATE auth_session SET ended_at = :now, end_reason = 'LOGOUT' WHERE session_id = :session_id AND ended_at IS NULL
