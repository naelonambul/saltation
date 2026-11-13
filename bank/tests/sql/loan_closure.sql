-- test.loan_closure: proves final loan state, repaid history and its correlated closure event.
-- Access path: ACCOUNT primary key, ACCOUNT_STATUS_HISTORY_IX and OUTBOX scan by account and request.
SELECT a.status,
(SELECT COUNT(*) FROM account_status_history h WHERE h.account_id = a.account_id AND h.to_status = 'CLOSED' AND h.reason = 'repaid'),
(SELECT COUNT(*) FROM outbox o WHERE JSON_VALUE(o.event_json, '$.event_type') = 'account.status_changed'
AND JSON_VALUE(o.event_json, '$.account_no') = a.account_no AND JSON_VALUE(o.event_json, '$.payload.to') = 'CLOSED'
AND JSON_VALUE(o.event_json, '$.payload.reason') = 'repaid' AND JSON_VALUE(o.event_json, '$.request_id') = :rid)
FROM account a WHERE a.account_id = :id
