-- test.count_events_for_request: outbox events produced by one request, by type.
SELECT LISTAGG(JSON_VALUE(event_json, '$.event_type'), ',') WITHIN GROUP (ORDER BY event_id)
FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :request_id
