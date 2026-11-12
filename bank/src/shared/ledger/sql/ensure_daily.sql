-- ledger.ensure_daily: creates the account's daily totals row if missing (account already locked).
-- Access path: DAILY_ACCOUNT_TOTAL primary key.
MERGE INTO daily_account_total d
USING (SELECT :account_id AS account_id, :business_date AS business_date FROM dual) s
ON (d.account_id = s.account_id AND d.business_date = s.business_date)
WHEN NOT MATCHED THEN INSERT (account_id, business_date) VALUES (s.account_id, s.business_date)
