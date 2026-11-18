-- test.auth_deferred_state: persisted activity and terminal guards for one token.
-- Access path: ACCESS_TOKEN and AUTH_SESSION primary keys.
SELECT t.expires_at, s.last_seen, t.revoked_at, s.ended_at
FROM access_token t JOIN auth_session s ON s.session_id = t.session_id
WHERE t.token_hash = :hash
