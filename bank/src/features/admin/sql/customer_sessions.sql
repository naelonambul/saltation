-- Recent sessions via AUTH_SESSION_CUSTOMER_IX.
WITH params AS (
  SELECT :customer_id AS customer_id,
         FROM_TZ(CAST(:after_time AS TIMESTAMP), '+09:00') AS after_time,
         :after_id AS after_id
  FROM dual
)
SELECT TO_CHAR(s.started_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM') AS key1, s.session_id AS key2,
  JSON_OBJECT(
    'session_id' VALUE s.session_id,
    'device_id' VALUE s.device_id,
    'started_at' VALUE TO_CHAR(s.started_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'last_seen' VALUE TO_CHAR(s.last_seen AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'ended_at' VALUE TO_CHAR(s.ended_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'end_reason' VALUE s.end_reason,
    'listener' VALUE s.listener
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM auth_session s
CROSS JOIN params
WHERE s.customer_id = params.customer_id
  AND (params.after_time IS NULL
       OR s.started_at < params.after_time
       OR (s.started_at = params.after_time AND (s.session_id COLLATE BINARY) < params.after_id))
ORDER BY s.started_at DESC, s.session_id COLLATE BINARY DESC
FETCH FIRST :take ROWS ONLY
