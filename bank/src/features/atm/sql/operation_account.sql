-- atm.operation_account: resolve the session account and read its current balance and status.
-- Access path: ACCOUNT_NO_UK.
SELECT account_id, balance, status FROM account WHERE account_no = :account
