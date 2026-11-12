-- auth.insert_session: a new authentication session.
-- Access path: AUTH_SESSION primary key.
INSERT INTO auth_session (session_id, customer_id, device_id, listener, human, ip, region, started_at, last_seen)
VALUES (:session_id, :customer_id, :device_id, :listener, :human, :ip, :region, :now, :now2)
