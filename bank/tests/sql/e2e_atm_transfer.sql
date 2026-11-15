-- test.e2e_atm_transfer: prove one ATM cash transfer and one posting on the smoke account.
-- Access path: TRANSFER_REQUEST_UK, ACCOUNT_TRANSACTION_TRANSFER_IX, ACCOUNT primary key.
SELECT JSON_OBJECT('kind' VALUE t.kind, 'amount' VALUE t.amount, 'rows' VALUE
 (SELECT COUNT(*) FROM account_transaction a WHERE a.transfer_id = t.transfer_id),
 'account_rows' VALUE (SELECT COUNT(*) FROM account_transaction a
 JOIN account c ON c.account_id = a.account_id
 WHERE a.transfer_id = t.transfer_id AND c.account_no = :account_no))
FROM transfer t WHERE t.request_id = :request_id
