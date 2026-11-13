-- test.reset_events: all events associated with the reset customer.
-- Access path: OUTBOX scan filtered by envelope customer_no.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.customer_no') = :customer_no ORDER BY event_id
