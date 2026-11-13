-- test.loan_requests: proves each money request's type, server channel and completion event.
-- Access path: TRANSACTION_REQUEST primary key and OUTBOX scan by request id.
SELECT r.request_type, r.channel,
(SELECT COUNT(*) FROM outbox o WHERE JSON_VALUE(o.event_json, '$.event_type') = 'request.completed'
AND JSON_VALUE(o.event_json, '$.request_id') = r.request_id)
FROM transaction_request r WHERE r.request_id = :request_id
