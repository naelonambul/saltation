-- test.close_delete_leg: daily close integration evidence.
-- Access path: test schema indexes and scans.
DELETE FROM account_transaction WHERE tran_id = :tran_id
