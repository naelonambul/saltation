-- atm.expiry_candidates: open sessions expired at the rollover cutoff.
-- Access path: ATM_SESSION_TERMINAL_OPEN_UK open-session index expression.
SELECT session_id, account_no, expires_at, request_id, terminal_id
FROM atm_session
WHERE CASE WHEN ended_at IS NULL THEN terminal_id END IS NOT NULL
  AND expires_at <= :at
