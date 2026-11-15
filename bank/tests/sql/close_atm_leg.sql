-- test.close_atm_leg: identify the ATM withdrawal ledger row by request ID.
-- Access path: TRANSFER_REQUEST_UK, ACCOUNT_TRANSACTION_TRANSFER_IX.
SELECT a.tran_id FROM transfer t
JOIN account_transaction a ON a.transfer_id = t.transfer_id
WHERE t.request_id = :id AND t.kind = 'atm_cash'
