-- Literal prefix with binary collation for both the match and the cursor.
WITH params AS (
  SELECT :pattern AS pattern, :after_name AS after_name, :after_id AS after_id FROM dual
)
SELECT c.name AS key1, TO_CHAR(c.customer_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key2,
  JSON_OBJECT(
    'customer_no' VALUE c.customer_no,
    'name' VALUE c.name,
    'phone' VALUE c.phone,
    'address' VALUE c.address,
    'customer_type' VALUE c.customer_type,
    'created_at' VALUE TO_CHAR(c.created_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM customer c
CROSS JOIN params
WHERE (c.name COLLATE BINARY) LIKE params.pattern ESCAPE '\'
  AND (params.after_id IS NULL
       OR (c.name COLLATE BINARY) > params.after_name
       OR ((c.name COLLATE BINARY) = params.after_name AND c.customer_id > params.after_id))
ORDER BY c.name COLLATE BINARY, c.customer_id
FETCH FIRST :take ROWS ONLY
