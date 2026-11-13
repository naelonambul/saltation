-- password_reset.revoke_tokens: revokes every unrevoked customer token after reset.
-- Access path: ACCESS_TOKEN_SUBJECT_IX range scan on the customer.
UPDATE access_token SET revoked_at = :now WHERE subject_type = 'CUSTOMER' AND subject_id = :customer_id AND revoked_at IS NULL
