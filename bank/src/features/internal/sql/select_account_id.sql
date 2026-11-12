-- internal.select_account_id: account ID by account number.
-- Access path: ACCOUNT_NO_UK.
SELECT account_id FROM account WHERE account_no = :account_no
