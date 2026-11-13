-- test.dormancy_events: read status events for one account.
-- Access path: OUTBOX scan on the fresh test schema.
SELECT JSON_SERIALIZE(event_json RETURNING VARCHAR2(4000)) FROM outbox
WHERE JSON_VALUE(event_json, '$.event_type') = 'account.status_changed'
AND JSON_VALUE(event_json, '$.account_no') = :account_no
ORDER BY event_id
