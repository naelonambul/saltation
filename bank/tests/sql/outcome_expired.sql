INSERT INTO access_token (token_hash, subject_type, subject_id, scope, session_id, expires_at)
SELECT :hash, subject_type, subject_id, scope, session_id, TO_TIMESTAMP_TZ('2000-01-01 00:00:00 +09:00', 'YYYY-MM-DD HH24:MI:SS TZH:TZM')
FROM access_token WHERE token_hash = :source
