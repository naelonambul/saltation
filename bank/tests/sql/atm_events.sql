-- test.atm_events: scan every persisted event for leaked secrets.
-- Access path: OUTBOX full scan.
SELECT event_json FROM outbox
