-- password_reset.end_sessions: ends every open customer session after reset.
-- Access path: AUTH_SESSION_CUSTOMER_IX by customer_id.
UPDATE auth_session SET ended_at = :now, end_reason = 'password_reset' WHERE customer_id = :customer_id AND ended_at IS NULL
