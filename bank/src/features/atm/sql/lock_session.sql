-- atm.lock_session: serialize token presentation.
-- Access path: ATM_SESSION token unique index.
SELECT session_id FROM atm_session WHERE token_hash = :hash FOR UPDATE
