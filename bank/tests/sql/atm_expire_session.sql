-- test.atm_expire_session: make a token expire without advancing business time.
-- Access path: ATM_SESSION token unique index.
UPDATE atm_session SET started_at = started_at - INTERVAL '10' MINUTE, expires_at = expires_at - INTERVAL '10' MINUTE WHERE token_hash = :hash AND ended_at IS NULL
