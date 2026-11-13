-- accounts.select_accounts: the customer's accounts with fintech numbers and products.
-- Access path: ACCOUNT_CUSTOMER_IX, then FINTECH_ACCOUNT_UK and product primary keys.
SELECT f.fintech_use_num, a.account_no, a.subtype, a.status, p.name, a.account_id
FROM account a
JOIN fintech_account f ON f.account_id = a.account_id
JOIN product_terms t ON t.terms_id = a.terms_id
JOIN product p ON p.product_id = t.product_id
WHERE a.customer_id = :customer_id
ORDER BY a.account_id
