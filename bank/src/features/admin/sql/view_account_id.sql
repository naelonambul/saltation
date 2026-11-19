-- admin.view_account_id: resolve the account independently of its activity.
SELECT account_id
FROM account
WHERE account_no = :account_no
