-- ledger.update_account: new balance and sequence after a movement (row already locked).
-- Access path: ACCOUNT primary key.
UPDATE account SET balance = :balance, next_sequence = :next_sequence, last_activity_at = CASE WHEN :customer_initiated = 1 THEN :now ELSE last_activity_at END
WHERE account_id = :account_id
