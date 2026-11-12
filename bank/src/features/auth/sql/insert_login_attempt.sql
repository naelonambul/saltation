-- auth.insert_login_attempt: every login attempt, including unknown phones.
-- Access path: LOGIN_ATTEMPT primary key (identity).
INSERT INTO login_attempt (phone, customer_id, outcome, device_id, ip, region, request_id, occurred_at)
VALUES (:phone, :customer_id, :outcome, :device_id, :ip, :region, :request_id, :now)
