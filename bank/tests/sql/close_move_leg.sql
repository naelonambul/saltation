-- test.close_move_leg: daily close integration evidence.
-- Access path: test schema indexes and scans.
UPDATE account_transaction SET account_id = (SELECT account_id FROM account WHERE account_no = :account_no), account_sequence = 999999 WHERE tran_id = :tran_id
