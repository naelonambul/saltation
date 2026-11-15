-- test.atm_account_state: inspect status and activity without changing them.
-- Access path: ACCOUNT_NO_UK.
SELECT JSON_OBJECT('status' VALUE status, 'last_activity_at' VALUE TO_CHAR(last_activity_at, 'YYYY-MM-DD HH24:MI:SS.FF6 TZH:TZM'))
FROM account WHERE account_no = :account
