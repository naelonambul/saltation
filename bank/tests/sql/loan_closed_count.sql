-- test.loan_closed_count: proves one terminal status transition for a dedicated loan.
-- Access path: ACCOUNT primary key and ACCOUNT_STATUS_HISTORY_IX.
SELECT a.status, (SELECT COUNT(*) FROM account_status_history h WHERE h.account_id = a.account_id AND h.to_status = 'CLOSED') FROM account a WHERE a.account_id = :id
