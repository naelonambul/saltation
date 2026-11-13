-- test.inbound_event: find completed request events for one inbound operation.
-- Access path: OUTBOX scan filtered by request_id and event_type in JSON.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request_id
AND JSON_VALUE(event_json, '$.event_type') = 'request.completed'
