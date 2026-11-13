-- test.loan_movements: proves both signed legs and request provenance for each loan movement.
-- Access path: ACCOUNT_TRANSACTION_DATE_IX then TRANSFER primary key and ACCOUNT_TRANSACTION_TRANSFER_IX.
WITH params AS (SELECT :id AS account_id, CAST(:rid AS VARCHAR2(26)) AS request_id FROM dual)
SELECT t.kind, COUNT(*), MIN(r.amount), MAX(r.amount), SUM(CASE WHEN r.request_id = t.request_id AND (p.request_id IS NULL OR t.request_id = p.request_id) THEN 2 ELSE 0 END)
FROM transfer t JOIN account_transaction r ON r.transfer_id = t.transfer_id CROSS JOIN params p
WHERE t.transfer_id IN (SELECT transfer_id FROM account_transaction WHERE account_id = p.account_id)
AND (p.request_id IS NULL OR t.request_id = p.request_id)
GROUP BY t.transfer_id, t.kind ORDER BY t.transfer_id
