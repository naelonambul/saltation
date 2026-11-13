-- deposits.close_info: reads linked account and early-close conditions.
-- Access path: ACCOUNT primary key, DEPOSIT_DETAIL primary key and PRODUCT_TERMS primary key.
SELECT d.linked_account_id, d.principal, t.early_close_ok, c.name FROM account a
JOIN deposit_detail d ON d.account_id = a.account_id JOIN product_terms t ON t.terms_id = a.terms_id
JOIN customer c ON c.customer_id = a.customer_id
WHERE a.account_id = :id AND a.customer_id = :customer AND a.subtype = 'TERM'
