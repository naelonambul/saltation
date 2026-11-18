-- cards.claim_payment: claim the client key before executing an authorization.
-- Access path: TRANSACTION_REQUEST primary key and TRANSACTION_REQUEST_IDEM_UK.
INSERT INTO transaction_request (
  request_id, request_type, channel, listener, idem_scope, idem_key, request_hash,
  status, rsp_code, device_id, ip, region, occurred_at
)
VALUES (:id, 'CARD_PAYMENT', 'CARD', :listener, :scope, :idem_key_value, :hash_value,
        'DONE', 'A0000', :device, :ip, :region, :now)
