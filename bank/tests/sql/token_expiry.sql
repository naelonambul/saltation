-- test.token_expiry: reads the persisted sliding expiry of a token.
-- Access path: ACCESS_TOKEN primary key.
SELECT expires_at FROM access_token WHERE token_hash = :hash
