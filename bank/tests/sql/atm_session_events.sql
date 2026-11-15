-- test.atm_session_events: inspect session end events.
-- Access path: OUTBOX event scan.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.payload.session_id') = :session_id AND JSON_VALUE(event_json, '$.event_type') = 'atm.session_ended' ORDER BY event_id
