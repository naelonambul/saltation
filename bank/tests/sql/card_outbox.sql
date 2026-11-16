SELECT COUNT(*) FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :1
