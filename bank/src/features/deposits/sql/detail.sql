-- deposits.detail: reads the customer deposit response.
-- Access path: ACCOUNT primary key, ownership filter, DEPOSIT_DETAIL and FINTECH_ACCOUNT_UK.
SELECT a.account_id, f.fintech_use_num, a.account_no, p.name, d.principal, a.balance,
TO_CHAR(a.opened_at AT TIME ZONE 'Asia/Seoul', 'YYYYMMDD'), TO_CHAR(d.maturity_date, 'YYYYMMDD'), a.status,
lf.fintech_use_num, d.close_type, TO_CHAR(d.closed_at AT TIME ZONE 'Asia/Seoul', 'YYYYMMDD')
FROM account a JOIN deposit_detail d ON d.account_id = a.account_id
JOIN fintech_account f ON f.account_id = a.account_id
JOIN fintech_account lf ON lf.account_id = d.linked_account_id
JOIN product_terms t ON t.terms_id = a.terms_id JOIN product p ON p.product_id = t.product_id
WHERE a.account_id = :id AND a.customer_id = :customer AND a.subtype = 'TERM'
