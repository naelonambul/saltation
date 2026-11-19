-- admin.insert_token: store only the hash of the issued analyst bearer.
-- Access path: ACCESS_TOKEN primary key and ACCESS_TOKEN_SUBJECT_IX.
INSERT INTO access_token (token_hash, subject_type, subject_id, scope, session_id, expires_at)
VALUES (:token_hash, 'ANALYST', :analyst_id, :read_scope, NULL, :expiry)
