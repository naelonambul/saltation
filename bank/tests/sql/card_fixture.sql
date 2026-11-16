DECLARE
  customer_key NUMBER;
  account_key NUMBER;
BEGIN
  INSERT INTO customer (customer_no, ci_hash, name, phone, customer_type, created_at)
  VALUES ('card-migrate', 'card-migrate', 'Card migration', 'card-migrate', 'INDIVIDUAL', SYSTIMESTAMP)
  RETURNING customer_id INTO customer_key;
  INSERT INTO account (account_no, customer_id, terms_id, subtype, status, opened_at)
  VALUES ('1700000000000001', customer_key, 1, 'DEMAND', 'ACTIVE', SYSTIMESTAMP)
  RETURNING account_id INTO account_key;
  INSERT INTO card (card_no, account_id, status, issued_at)
  VALUES ('0000000000000000', account_key, 'ACTIVE', SYSTIMESTAMP);
  INSERT INTO transaction_request (request_id, request_type, channel, listener, status, rsp_code, occurred_at)
  VALUES ('card-migration-request', 'CARD', 'CARD', 'GENERATOR', 'REJECTED', 'A0004', SYSTIMESTAMP);
END;
