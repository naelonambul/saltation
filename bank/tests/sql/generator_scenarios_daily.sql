SELECT TO_CHAR(d.business_date, 'YYYY-MM-DD'), d.cash_in, d.cash_out, d.transfer_out
FROM daily_account_total d
JOIN account a ON a.account_id = d.account_id
WHERE a.account_no = :account_no
ORDER BY d.business_date
