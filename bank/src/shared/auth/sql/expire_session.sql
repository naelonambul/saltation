UPDATE auth_session
SET ended_at = last_seen + INTERVAL '30' MINUTE, end_reason = 'expired'
WHERE session_id = :session_id AND ended_at IS NULL AND last_seen <= :cutoff
