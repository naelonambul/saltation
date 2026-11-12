-- auth.touch_session: records session activity.
-- Access path: AUTH_SESSION primary key.
UPDATE auth_session SET last_seen = :now WHERE session_id = :session_id
