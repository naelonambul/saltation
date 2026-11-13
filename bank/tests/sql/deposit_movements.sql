-- test.deposit_movements: proves deposit movement shape, dates and request provenance.
-- Access path: ACCOUNT_TRANSACTION_DATE_IX then TRANSFER primary key and ACCOUNT_TRANSACTION_TRANSFER_IX.
SELECT t.kind, COUNT(*), SUM(r.amount), TO_CHAR(MIN(r.business_date), 'YYYYMMDD'),
TO_CHAR(MAX(r.business_date), 'YYYYMMDD'), COUNT(r.request_id) + COUNT(t.request_id)
FROM transfer t JOIN account_transaction r ON r.transfer_id = t.transfer_id
WHERE t.transfer_id IN (SELECT transfer_id FROM account_transaction WHERE account_id = :id)
GROUP BY t.transfer_id, t.kind ORDER BY t.transfer_id
