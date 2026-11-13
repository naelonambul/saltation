-- ledger.lock_daily: the account's totals for a business date; zeros when it has none yet.
-- Access path: DAILY_ACCOUNT_TOTAL primary key. The caller's ACCOUNT row lock serializes writers.
SELECT NVL(MAX(transfer_out), 0), NVL(MAX(cash_in), 0), NVL(MAX(cash_out), 0) FROM daily_account_total
WHERE account_id = :account_id AND business_date = :business_date
