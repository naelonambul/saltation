SELECT c.customer_no, a.balance
FROM account a
JOIN customer c ON c.customer_id = a.customer_id
WHERE a.account_no = :account_no
