-- One selector is bound. Each branch looks up its own unique index.
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
WHERE c.customer_id IN (
  SELECT customer_id FROM customer WHERE phone = :phone
  UNION ALL
  SELECT customer_id FROM customer WHERE customer_no = :customer_no
)
