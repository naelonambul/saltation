-- auth.select_token: a live customer token and its session.
-- Access path: ACCESS_TOKEN primary key, then CUSTOMER primary key.
SELECT t.subject_id, c.customer_no, t.scope, t.session_id, s.device_id
FROM access_token t JOIN customer c ON c.customer_id = t.subject_id
JOIN auth_session s ON s.session_id = t.session_id
WHERE t.token_hash = :token_hash
  AND t.subject_type = 'CUSTOMER'
  AND t.revoked_at IS NULL
  AND t.expires_at > :now
