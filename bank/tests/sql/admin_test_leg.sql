-- test.admin_test_leg: locate a real ledger row by the operation request.
SELECT tran_id FROM account_transaction WHERE request_id = :request_id
