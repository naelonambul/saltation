-- test.admin_test_customer: return the profile identifier and phone for a seeded account.
SELECT c.customer_no, c.phone FROM customer c JOIN account a ON a.customer_id = c.customer_id
WHERE a.account_no = :account_no
