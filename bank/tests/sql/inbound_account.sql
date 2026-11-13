-- test.inbound_account: read balance and activity to prove deposit effects without changing time.
-- Access path: ACCOUNT_NO_UK.
SELECT account_id, balance, status, last_activity_at FROM account WHERE account_no = :account
