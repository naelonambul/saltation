-- test.payee_account_id: locate the test recipient for a lifecycle transition.
-- Access path: ACCOUNT_NO_UK.
SELECT account_id FROM account WHERE account_no = :account_no
