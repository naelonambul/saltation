-- test.atm_lock_session_nowait: prove an earlier session lock was released.
-- Access path: ATM_SESSION token unique index.
SELECT session_id FROM atm_session WHERE token_hash = :hash FOR UPDATE NOWAIT
