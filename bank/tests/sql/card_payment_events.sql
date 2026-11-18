-- test.card_payment_events: read outbox events correlated with the HTTP request.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.request_id') = :id ORDER BY event_id
