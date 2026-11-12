-- auth.extend_token: slides the token expiry (30 minutes of business-time inactivity).
-- Access path: ACCESS_TOKEN primary key.
UPDATE access_token SET expires_at = :expires_at WHERE token_hash = :token_hash
