-- internal.insert_account: a seeded demand deposit account (terms 1, balance 0).
-- Access path: ACCOUNT_NO_UK.
INSERT INTO account (account_no, customer_id, terms_id, subtype, status, opened_at)
VALUES (:account_no, :customer_id, 1, 'DEMAND', 'ACTIVE', :now)
RETURNING account_id INTO :id
