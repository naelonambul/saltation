-- atm.insert_session: create a bearer session storing only its hash.
-- Access path: ATM_SESSION primary key and token and open terminal unique indexes.
INSERT INTO atm_session (session_id, terminal_id, account_no, token_hash, started_at, expires_at, request_id) VALUES (:id, :terminal, :account, :hash, :started, :expires, :request)
