-- test.admin_token_expiry: move an integration token's recorded deadline.
UPDATE access_token SET expires_at = :expiry WHERE token_hash = :token_hash
