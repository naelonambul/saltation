-- Lookup by CUSTOMER_NO_UK.
SELECT JSON_OBJECT(
    'customer_no' VALUE c.customer_no,
    'name' VALUE c.name,
    'phone' VALUE c.phone,
    'address' VALUE c.address,
    'customer_type' VALUE c.customer_type,
    'created_at' VALUE TO_CHAR(c.created_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM customer c
WHERE c.customer_no = :customer_no
