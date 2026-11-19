-- test.admin_read_events: persisted audit by HTTP correlation ID.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request_id ORDER BY event_id
