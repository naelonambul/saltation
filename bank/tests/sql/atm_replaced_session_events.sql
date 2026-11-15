-- test.atm_replaced_session_events: find every end event for the replaced session.
-- Access path: OUTBOX full scan (test helper).
SELECT event_json FROM outbox
WHERE JSON_VALUE(event_json, '$.event_type') = 'atm.session_ended'
  AND JSON_VALUE(event_json, '$.payload.session_id') = :session_id
ORDER BY event_id
