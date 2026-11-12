-- accounts.select_balance: one of the customer's accounts by fintech number.
-- Access path: FINTECH_ACCOUNT and ACCOUNT primary keys; account_transaction_date_ix for latest date.
SELECT a.account_id, a.account_no, a.balance, a.subtype, p.name, a.opened_at, (SELECT TO_CHAR(MAX(tx.business_date), 'YYYYMMDD')
       FROM account_transaction tx WHERE tx.account_id = a.account_id)
FROM fintech_account f
JOIN account a ON a.account_id = f.account_id
JOIN product_terms t ON t.terms_id = a.terms_id
JOIN product p ON p.product_id = t.product_id
WHERE f.fintech_use_num = :fintech_use_num AND a.customer_id = :customer_id
