-- ledger.add_daily: adds to the account's daily totals, creating the row on the first movement.
-- Access path: DAILY_ACCOUNT_TOTAL primary key. The caller holds the ACCOUNT row lock.
MERGE INTO daily_account_total d
USING (SELECT :account_id AS account_id, :business_date AS business_date, :transfer_out AS transfer_out,
              :cash_in AS cash_in, :cash_out AS cash_out FROM dual) s
ON (d.account_id = s.account_id AND d.business_date = s.business_date)
WHEN MATCHED THEN UPDATE SET d.transfer_out = d.transfer_out + s.transfer_out,
  d.cash_in = d.cash_in + s.cash_in, d.cash_out = d.cash_out + s.cash_out
WHEN NOT MATCHED THEN INSERT (account_id, business_date, transfer_out, cash_in, cash_out)
  VALUES (s.account_id, s.business_date, s.transfer_out, s.cash_in, s.cash_out)
