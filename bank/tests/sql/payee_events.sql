-- test.payee_events: verify recipient persistence.
-- Access path: OUTBOX scan filtered by payee id.
SELECT event_json FROM outbox WHERE JSON_VALUE(event_json, '$.payload.payee_id') = :payee_id AND JSON_VALUE(event_json, '$.event_type') IN ('payee.registered', 'payee.deleted') ORDER BY event_id
