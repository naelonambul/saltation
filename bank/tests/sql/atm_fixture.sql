-- test.atm_fixture
DECLARE
  fixture_customer NUMBER;
BEGIN
  INSERT INTO customer (customer_no, ci_hash, name, phone, customer_type, created_at)
  VALUES ('atm-migrate', 'atm-migrate', 'ATM migration', 'atm-migrate', 'INDIVIDUAL', SYSTIMESTAMP)
  RETURNING customer_id INTO fixture_customer;
  INSERT INTO account (account_no, customer_id, terms_id, subtype, status, opened_at)
  VALUES ('1600000000000001', fixture_customer, 1, 'DEMAND', 'ACTIVE', SYSTIMESTAMP);
  INSERT INTO atm_terminal (terminal_id, region, location, latitude, longitude)
  VALUES ('migration16', 'KR-11', 'Migration test', 37.5, 127.0);
  INSERT INTO atm_credential (account_no, pin_hash)
  VALUES ('1600000000000001', 'test-hash');
END;
