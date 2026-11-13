-- test.inbound_token: inspect the client token subject, session and expiry.
-- Access path: ACCESS_TOKEN primary key then COUNTERPARTY_CLIENT primary key.
SELECT t.subject_type, c.client_id, t.scope, t.session_id, t.expires_at
FROM access_token t JOIN counterparty_client c ON c.counterparty_client_id = t.subject_id
WHERE t.token_hash = :hash
