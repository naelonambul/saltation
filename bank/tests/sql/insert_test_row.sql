-- test.insert_test_row: a transaction row inserted directly (step 6 runs before the ledger exists).
INSERT INTO account_transaction (account_id, account_sequence, amount, balance_after, inout_type, tran_type, print_content, channel, request_id, business_date, occurred_at)
VALUES (:account_id, :seq, :amount, :balance_after, :inout_type, '대체', :print_content, 'MOBILE', :request_id, TRUNC(:occurred_at), :occurred_at2)
