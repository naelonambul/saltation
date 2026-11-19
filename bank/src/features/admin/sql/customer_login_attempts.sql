-- Recent attempts via LOGIN_ATTEMPT_PHONE_IX with verified customer attribution.
WITH params AS (
  SELECT :customer_id AS customer_id, :phone AS phone,
         FROM_TZ(CAST(:after_time AS TIMESTAMP), '+09:00') AS after_time,
         :after_id AS after_id
  FROM dual
)
SELECT TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM') AS key1, TO_CHAR(l.attempt_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key2,
  JSON_OBJECT(
    'attempt_id' VALUE TO_CHAR(l.attempt_id, 'TM9'),
    'outcome' VALUE l.outcome,
    'occurred_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'recorded_at' VALUE TO_CHAR(l.recorded_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'request_id' VALUE l.request_id
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM login_attempt l
CROSS JOIN params
WHERE l.phone = params.phone
  AND l.customer_id = params.customer_id
  AND (params.after_time IS NULL OR (
    l.occurred_at <= params.after_time
    AND (l.occurred_at < params.after_time
         OR (l.occurred_at = params.after_time AND l.attempt_id < params.after_id))
  ))
ORDER BY l.occurred_at DESC, l.attempt_id DESC
FETCH FIRST :take ROWS ONLY
