CREATE INDEX auth_session_open_last_seen_ix ON auth_session (CASE WHEN ended_at IS NULL THEN last_seen END)
