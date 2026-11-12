-- outbox_relay.select_batch: the oldest undelivered events.
-- Access path: OUTBOX_PK (event_id order).
SELECT event_id, event_json FROM outbox ORDER BY event_id FETCH FIRST :batch_size ROWS ONLY
