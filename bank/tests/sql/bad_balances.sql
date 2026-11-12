-- test.bad_balances: accounts whose stored balance differs from the sum of their rows.
SELECT COUNT(*) FROM account a
WHERE a.balance != NVL((SELECT SUM(r.amount) FROM account_transaction r WHERE r.account_id = a.account_id), 0)
