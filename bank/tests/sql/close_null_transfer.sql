-- test.close_null_transfer: daily close integration evidence.
-- Access path: test schema indexes and scans.
UPDATE account_transaction SET transfer_id = NULL WHERE tran_id = :tran_id
