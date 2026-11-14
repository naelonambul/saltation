-- atm.expiry_candidates: open sessions expired at the rollover cutoff.
-- Access path: ATM_SESSION_OPEN_EXPIRY_IX (the predicate repeats the index expression).
SELECT session_id, account_no, expires_at, request_id, terminal_id
FROM atm_session
WHERE CASE WHEN ended_at IS NULL THEN expires_at END <= :at
