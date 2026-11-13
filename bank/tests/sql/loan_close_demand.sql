-- test.loan_close_demand: closes a dedicated demand account for guard assertions.
-- Access path: FINTECH_ACCOUNT primary key then ACCOUNT primary key.
UPDATE account SET status = 'CLOSED' WHERE subtype = 'DEMAND' AND account_id = (SELECT account_id FROM fintech_account WHERE fintech_use_num = :fin)
