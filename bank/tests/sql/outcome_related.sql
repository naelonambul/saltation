SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request_id ORDER BY event_id
