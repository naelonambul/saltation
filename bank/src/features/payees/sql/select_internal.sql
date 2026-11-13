-- payees.select_internal: an in-bank account's holder name, status and subtype.
-- Access path: ACCOUNT_NO_UK then CUSTOMER primary key.
SELECT c.name, a.status, a.subtype FROM account a JOIN customer c ON c.customer_id = a.customer_id WHERE a.account_no = :account_no
