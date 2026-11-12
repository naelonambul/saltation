-- ledger.insert_status: record a locked account status transition.
-- Access path: ACCOUNT_STATUS_HISTORY insert, ACCOUNT foreign key.
INSERT INTO account_status_history (account_id, from_status, to_status, reason, occurred_at)
VALUES (:account_id, :from_status, :to_status, :reason, :occurred_at)
