-- auth.revoke_token: revokes a live token.
-- Access path: ACCESS_TOKEN primary key.
UPDATE access_token SET revoked_at = :now WHERE token_hash = :token_hash AND revoked_at IS NULL
