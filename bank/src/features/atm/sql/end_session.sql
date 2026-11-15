-- atm.end_session: transition an open session exactly once.
-- Access path: ATM_SESSION primary key.
UPDATE atm_session SET ended_at = :ended, end_reason = :reason WHERE session_id = :id AND ended_at IS NULL
