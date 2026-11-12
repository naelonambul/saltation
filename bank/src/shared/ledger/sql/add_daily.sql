-- ledger.add_daily: adds to the account's daily totals.
-- Access path: DAILY_ACCOUNT_TOTAL primary key.
UPDATE daily_account_total
SET transfer_out = transfer_out + :transfer_out, cash_in = cash_in + :cash_in, cash_out = cash_out + :cash_out
WHERE account_id = :account_id AND business_date = :business_date
