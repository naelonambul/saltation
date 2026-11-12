-- transfers.select_receiver: an in-bank account by account number.
-- Access path: ACCOUNT_NO_UK, then CUSTOMER primary key.
SELECT a.account_id, c.customer_no, c.name
FROM account a JOIN customer c ON c.customer_id = a.customer_id
WHERE a.account_no = :account_no
