-- test.admin_token_revoke: revoke the separate positive-control bearer.
UPDATE access_token SET revoked_at = :wall_now WHERE token_hash = :token_hash
