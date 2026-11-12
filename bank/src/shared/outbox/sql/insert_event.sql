-- outbox.insert_event: one business event, in the caller's transaction.
-- Access path: OUTBOX primary key (OUTBOX_TS).
INSERT INTO outbox (event_id, event_json) VALUES (:event_id, :event_json)
