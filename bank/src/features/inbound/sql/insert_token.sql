-- inbound.insert_token: store a client token without a customer session.
-- Access path: ACCESS_TOKEN primary key.
INSERT INTO access_token (token_hash, subject_type, subject_id, scope, session_id, expires_at) VALUES (:hash, 'CLIENT', :id, :scope, NULL, :expires)
