-- loans.repay_info: finds the owned loan and its linked repayment account.
-- Access path: ACCOUNT primary key and LOAN_DETAIL primary key with ownership filter.
SELECT d.linked_account_id, c.name FROM account a JOIN loan_detail d ON d.account_id = a.account_id
JOIN customer c ON c.customer_id = a.customer_id
WHERE a.account_id = :id AND a.customer_id = :customer AND a.subtype = 'LOAN'
