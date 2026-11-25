-- atm.lock_session: serialize token presentation.
-- Access path: ATM_SESSION token unique index.
SELECT session_id, account_no, expires_at, ended_at, request_id, terminal_id FROM atm_session WHERE token_hash = :hash FOR UPDATE WAIT 1
