-- test.close_leg: daily close integration evidence.
-- Access path: test schema indexes and scans.
SELECT r.tran_id FROM account_transaction r JOIN account a ON a.account_id = r.account_id
 WHERE a.account_no = :account_no AND r.occurred_at >= :start_at AND r.occurred_at < :end_at ORDER BY r.account_sequence
