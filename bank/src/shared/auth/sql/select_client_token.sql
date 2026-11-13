-- auth.select_client_token: authenticate an unrevoked unexpired client bearer without touching sessions.
-- Access path: ACCESS_TOKEN primary key then COUNTERPARTY_CLIENT primary key.
SELECT c.counterparty_client_id, c.client_id, c.institution_code, t.scope
FROM access_token t JOIN counterparty_client c ON c.counterparty_client_id = t.subject_id
WHERE t.token_hash = :hash AND t.subject_type = 'CLIENT' AND t.revoked_at IS NULL AND t.expires_at > :now
