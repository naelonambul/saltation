-- test.set_balance: sets an account's balance and next sequence after direct inserts.
UPDATE account SET balance = :balance, next_sequence = :next_seq, last_activity_at = :now WHERE account_no = :account_no
