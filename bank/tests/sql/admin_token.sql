-- test.admin_token: confirm only the bearer digest identifies the token.
SELECT token_hash, subject_type, scope, session_id, expires_at
FROM access_token WHERE token_hash = :token_hash
