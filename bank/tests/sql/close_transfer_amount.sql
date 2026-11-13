-- test.close_transfer_amount: daily close integration evidence.
-- Access path: test schema indexes and scans.
UPDATE transfer SET amount = amount + :delta WHERE transfer_id = (SELECT transfer_id FROM account_transaction WHERE tran_id = :tran_id)
