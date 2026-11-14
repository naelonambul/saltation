-- atm.operation_account: resolve the session account and read its current balance.
-- Access path: ACCOUNT_NO_UK.
SELECT account_id, balance FROM account WHERE account_no = :account
