-- test.bad_running_balances: rows whose balance_after is not the running sum in sequence order.
SELECT COUNT(*) FROM (
  SELECT balance_after, SUM(amount) OVER (PARTITION BY account_id ORDER BY account_sequence) AS running
  FROM account_transaction)
WHERE balance_after != running
