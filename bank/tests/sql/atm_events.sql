-- test.atm_events: scan events created after the test watermark for leaked secrets.
-- Access path: OUTBOX primary-key range scan.
SELECT event_json FROM outbox WHERE event_id > :watermark
