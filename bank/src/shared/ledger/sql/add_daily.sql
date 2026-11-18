-- ledger.add_daily: adds to the account's daily totals, creating the row on the first movement.
-- Access path: DAILY_ACCOUNT_TOTAL primary key on both columns. The caller holds the ACCOUNT row lock.
-- TRUNC(datetime) returns a Gregorian DATE independently of session NLS_CALENDAR.
MERGE INTO daily_account_total d
USING dual
ON (d.account_id = :account_id AND d.business_date = TRUNC(:business_date))
WHEN MATCHED THEN UPDATE SET d.transfer_out = d.transfer_out + :transfer_out,
  d.cash_in = d.cash_in + :cash_in, d.cash_out = d.cash_out + :cash_out
WHEN NOT MATCHED THEN INSERT (account_id, business_date, transfer_out, cash_in, cash_out)
  VALUES (:account_id, TRUNC(:business_date), :transfer_out, :cash_in, :cash_out)
