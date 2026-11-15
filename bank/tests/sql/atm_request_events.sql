-- test.atm_request_events: inspect request events in creation order.
-- Access path: OUTBOX full scan filtered by $.request_id (test helper).
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request ORDER BY event_id
