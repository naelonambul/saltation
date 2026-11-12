-- test.bad_transfers: transfer records whose two rows do not offset (spec F-21).
SELECT COUNT(*) FROM (
  SELECT t.transfer_id FROM transfer t JOIN account_transaction r ON r.transfer_id = t.transfer_id
  WHERE t.kind = 'transfer'
  GROUP BY t.transfer_id
  HAVING SUM(r.amount) != 0 OR COUNT(*) != 2)
