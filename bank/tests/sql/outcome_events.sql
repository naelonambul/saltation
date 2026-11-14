-- Completion events for one request, in creation order.
SELECT event_json FROM outbox
WHERE JSON_VALUE(event_json, '$.request_id') = :request_id
AND JSON_VALUE(event_json, '$.event_type') = 'request.completed'
ORDER BY event_id
