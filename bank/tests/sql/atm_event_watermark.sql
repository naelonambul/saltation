-- test.atm_event_watermark: capture the last persisted event before test actions.
-- Access path: OUTBOX primary-key maximum. ULIDs use a string zero for an empty table.
SELECT COALESCE(MAX(event_id), '0') FROM outbox
