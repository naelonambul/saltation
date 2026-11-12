-- test.sum_balances: total balance of accounts whose number starts with a prefix.
SELECT NVL(SUM(balance), 0) FROM account WHERE account_no LIKE :prefix
