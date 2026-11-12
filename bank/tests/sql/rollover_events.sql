-- test.rollover_events: reads completion events in insertion order.
-- Access path: OUTBOX full scan on the fresh test schema, ordered by event_id.
SELECT JSON_SERIALIZE(event_json RETURNING VARCHAR2(4000)) FROM outbox
WHERE JSON_VALUE(event_json, '$.event_type') = 'rollover.job_completed'
AND JSON_VALUE(event_json, '$.payload.business_date') >= :business_date
AND JSON_VALUE(event_json, '$.payload.job') IN ('first', 'second')
ORDER BY event_id
