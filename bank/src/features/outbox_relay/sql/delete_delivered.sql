-- outbox_relay.delete_delivered: removes one delivered event (executed as array DML).
-- Access path: OUTBOX_PK. Explicit IDs, never a range: a transaction that
-- commits later may hold a smaller event_id that is not yet delivered.
DELETE FROM outbox WHERE event_id = :event_id
