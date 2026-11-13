-- deposits.payer: finds a customer-owned funding account.
-- Access path: FINTECH_ACCOUNT primary key then ACCOUNT primary key.
SELECT a.account_id FROM fintech_account f JOIN account a ON a.account_id = f.account_id
WHERE f.fintech_use_num = :fin AND a.customer_id = :customer
