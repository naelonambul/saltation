-- test.close_consistent: daily close integration evidence.
-- Access path: test schema indexes and scans.
SELECT
 (SELECT COUNT(*) FROM account a WHERE a.account_no = :account_no AND a.balance <> NVL((SELECT SUM(r.amount) FROM account_transaction r WHERE r.account_id = a.account_id), 0)) +
 (SELECT COUNT(*) FROM (
 SELECT r.balance_after, SUM(r.amount) OVER (ORDER BY r.account_sequence) running
 FROM account_transaction r JOIN account a ON a.account_id = r.account_id WHERE a.account_no = :account_no
 ) WHERE balance_after <> running) +
 (SELECT COUNT(*) FROM (
 SELECT t.transfer_id FROM transfer t JOIN account_transaction r ON r.transfer_id = t.transfer_id
 WHERE t.kind = 'transfer' AND EXISTS (
 SELECT 1 FROM account_transaction x JOIN account a ON a.account_id = x.account_id
 WHERE x.transfer_id = t.transfer_id AND a.account_no = :account_no)
 GROUP BY t.transfer_id, t.amount
 HAVING CASE WHEN COUNT(*) = 2 AND SUM(r.amount) = 0 THEN 0
 WHEN COUNT(*) = 1 AND SUM(r.amount) = -t.amount AND MAX(r.counterparty_bank_code) IN
 (SELECT institution_code FROM institution WHERE institution_type = 'BANK') THEN 0 ELSE 1 END = 1)
 ) FROM dual
