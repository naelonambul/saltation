-- test.atm_insert_session
INSERT INTO atm_session (session_id, terminal_id, account_no, token_hash, started_at, expires_at, request_id)
VALUES (:session_id, 'migration16', '1600000000000001', :token_hash, SYSTIMESTAMP, SYSTIMESTAMP + INTERVAL '3' MINUTE, :request_id)
