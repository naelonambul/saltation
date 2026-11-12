-- test.dormancy_account: read account state and activity.
-- Access path: ACCOUNT_NO_UK.
SELECT status, balance, last_activity_at FROM account WHERE account_no = :account_no
