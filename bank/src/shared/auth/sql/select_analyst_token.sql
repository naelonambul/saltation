-- auth.select_analyst_token: validate the analyst subject, scope and fixed wall-clock expiry.
-- Access path: ACCESS_TOKEN primary key, then ANALYST primary key.
SELECT a.analyst_id FROM access_token t JOIN analyst a ON a.analyst_id = t.subject_id
WHERE t.token_hash = :token_hash AND t.subject_type = 'ANALYST'
  AND t.scope = :read_scope AND t.session_id IS NULL
  AND t.revoked_at IS NULL AND t.expires_at > :wall_now
