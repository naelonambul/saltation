-- test.dormancy_history: read status transitions, excluding the opening record.
-- Access path: ACCOUNT_NO_UK and ACCOUNT_STATUS_HISTORY_IX.
SELECT h.from_status, h.to_status, h.reason, h.occurred_at
FROM account_status_history h JOIN account a ON a.account_id = h.account_id
WHERE a.account_no = :account_no AND h.from_status IS NOT NULL
ORDER BY h.occurred_at
