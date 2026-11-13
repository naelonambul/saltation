-- inbound.select_receiver: resolve the receiving account and customer by fintech number.
-- Access path: FINTECH_ACCOUNT primary key then ACCOUNT and CUSTOMER primary keys.
SELECT a.account_id, a.account_no, c.customer_no, c.name FROM fintech_account f JOIN account a ON a.account_id = f.account_id JOIN customer c ON c.customer_id = a.customer_id WHERE f.fintech_use_num = :fin
