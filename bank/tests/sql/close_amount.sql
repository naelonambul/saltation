-- test.close_amount: daily close integration evidence.
-- Access path: test schema indexes and scans.
UPDATE account_transaction SET amount = amount + :delta WHERE tran_id = :tran_id
