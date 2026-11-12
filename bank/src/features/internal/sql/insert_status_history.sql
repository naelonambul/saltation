-- internal.insert_status_history: the opening status of a seeded account.
-- Access path: ACCOUNT_STATUS_HISTORY_IX.
INSERT INTO account_status_history (account_id, from_status, to_status, reason, occurred_at)
VALUES (:account_id, NULL, 'ACTIVE', 'OPENED', :now)
