-- 0014_request_attempts: rejected attempts may omit the idempotency claim.

ALTER TABLE transaction_request MODIFY (
  idem_scope NULL,
  idem_key NULL,
  request_hash NULL
)
/
ALTER TABLE transaction_request ADD CONSTRAINT transaction_request_idem_ck CHECK (
  (idem_scope IS NOT NULL AND idem_key IS NOT NULL AND request_hash IS NOT NULL)
  OR (idem_scope IS NULL AND idem_key IS NULL AND request_hash IS NULL AND status = 'REJECTED')
)
