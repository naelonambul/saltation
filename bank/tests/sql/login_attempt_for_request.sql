-- test.login_attempt_for_request: outcome and whether a customer was resolved.
SELECT outcome, NVL2(customer_id, 'Y', 'N') FROM login_attempt WHERE request_id = :request_id
