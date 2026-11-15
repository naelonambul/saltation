-- test.atm_cash_totals: inspect totals for the business date of the latest cash row.
-- Access path: ACCOUNT_NO_UK, DAILY_ACCOUNT_TOTAL primary key.
SELECT JSON_OBJECT('cash_in' VALUE d.cash_in, 'cash_out' VALUE d.cash_out)
FROM daily_account_total d JOIN account a ON a.account_id = d.account_id
WHERE a.account_no = :account AND d.business_date = (
 SELECT MAX(t.business_date) FROM account_transaction t WHERE t.account_id = a.account_id AND t.channel = 'ATM')
