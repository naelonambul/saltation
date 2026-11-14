-- test.atm_request_events: inspect request events in creation order.
-- Access path: OUTBOX event index.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request ORDER BY event_id
