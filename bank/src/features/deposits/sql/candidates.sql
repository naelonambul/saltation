-- deposits.candidates: finds outstanding matured deposits and their holder.
-- Access path: DEPOSIT_DETAIL_MATURITY_IX date range then ACCOUNT and CUSTOMER primary keys.
SELECT d.account_id, d.linked_account_id, d.principal, c.name FROM deposit_detail d
JOIN account a ON a.account_id = d.account_id JOIN customer c ON c.customer_id = a.customer_id
WHERE d.maturity_date <= :business_date AND d.closed_at IS NULL ORDER BY d.account_id
