-- internal.insert_counterparty: seed a simulated external recipient.
-- Access path: COUNTERPARTY_ACCOUNT_UK; INSTITUTION primary key foreign key.
INSERT INTO counterparty (counterparty_type, industry_code, institution_code, account_no, name, created_at)
VALUES (:counterparty_type, :industry_code, :institution_code, :account_no, :name, :created_at)
