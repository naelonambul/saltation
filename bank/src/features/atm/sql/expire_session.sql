-- atm.expire_session: expire an open session exactly once at its stored expiry.
-- Access path: ATM_SESSION primary key.
UPDATE atm_session SET ended_at = expires_at, end_reason = 'expired'
WHERE session_id = :id AND ended_at IS NULL
