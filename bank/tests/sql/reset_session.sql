-- test.reset_session: verifies reset closes and revokes the original login.
-- Access path: ACCESS_TOKEN primary key, then AUTH_SESSION primary key.
SELECT s.end_reason, CASE WHEN s.ended_at IS NOT NULL THEN 1 ELSE 0 END, CASE WHEN t.revoked_at IS NOT NULL THEN 1 ELSE 0 END FROM access_token t JOIN auth_session s ON s.session_id = t.session_id WHERE t.token_hash = :token_hash
