-- ledger.lock_daily: the account's totals for a business date, locked.
-- Access path: DAILY_ACCOUNT_TOTAL primary key.
SELECT transfer_out, cash_in, cash_out FROM daily_account_total
WHERE account_id = :account_id AND business_date = :business_date FOR UPDATE WAIT 1
