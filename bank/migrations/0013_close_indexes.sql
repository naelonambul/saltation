-- 0013_close_indexes: timestamp access paths for daily reconciliation.

CREATE INDEX transfer_occurred_ix ON transfer (occurred_at, transfer_id)
/
CREATE INDEX account_transaction_occurred_ix ON account_transaction (occurred_at, account_id, account_sequence)
