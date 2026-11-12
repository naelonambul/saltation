-- auth.insert_token: a customer access token (stored as a hash).
-- Access path: ACCESS_TOKEN primary key.
INSERT INTO access_token (token_hash, subject_type, subject_id, scope, session_id, expires_at)
VALUES (:token_hash, 'CUSTOMER', :customer_id, :scope, :session_id, :expires_at)
