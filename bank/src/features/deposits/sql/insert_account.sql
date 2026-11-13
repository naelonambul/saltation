-- deposits.insert_account: creates a zero-balance term account.
-- Access path: ACCOUNT_NO_UK; identity assigns the account primary key.
INSERT INTO account (account_no, customer_id, terms_id, subtype, status, opened_at)
VALUES (:placeholder, :customer, :terms, 'TERM', 'ACTIVE', :at)
RETURNING account_id INTO :id
