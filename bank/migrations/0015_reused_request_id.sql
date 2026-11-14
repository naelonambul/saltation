-- 0015_reused_request_id: preserve the client ID on rejected reused-ID attempts.

ALTER TABLE transaction_request ADD (reused_request_id VARCHAR2(26))
