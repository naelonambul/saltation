-- test.ledger_tune_date: typed DATE column round-trips without NLS string formatting.
SELECT business_date FROM daily_account_total WHERE account_id = :account_id AND business_date = :business_date
