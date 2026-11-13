-- test.deposit_closure: proves persisted closure, history and its status event.
-- Access path: DEPOSIT_DETAIL primary key, ACCOUNT_STATUS_HISTORY_IX and OUTBOX scan.
SELECT d.close_type,
(SELECT COUNT(*) FROM account_status_history h WHERE h.account_id = d.account_id AND h.to_status = 'CLOSED' AND h.reason = CASE d.close_type WHEN 'EARLY' THEN 'early_close' ELSE 'maturity' END),
(SELECT COUNT(*) FROM outbox o WHERE JSON_VALUE(o.event_json, '$.event_type') = 'account.status_changed'
AND JSON_VALUE(o.event_json, '$.account_no') = a.account_no AND JSON_VALUE(o.event_json, '$.payload.to') = 'CLOSED')
FROM deposit_detail d JOIN account a ON a.account_id = d.account_id WHERE d.account_id = :id
