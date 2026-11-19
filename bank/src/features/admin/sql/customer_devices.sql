-- Ownership and device order use DEVICE_PK.
WITH params AS (
  SELECT :customer_id AS customer_id, :after_id AS after_id FROM dual
)
SELECT d.device_id AS key1, '-' AS key2,
  JSON_OBJECT(
    'device_id' VALUE d.device_id,
    'first_seen' VALUE TO_CHAR(d.first_seen AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'last_seen' VALUE TO_CHAR(d.last_seen AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM device d
CROSS JOIN params
WHERE d.customer_id = params.customer_id
  AND (params.after_id IS NULL OR (d.device_id COLLATE BINARY) > params.after_id)
ORDER BY d.device_id COLLATE BINARY
FETCH FIRST :take ROWS ONLY
