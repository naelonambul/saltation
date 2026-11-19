-- Active payees via PAYEE_CUSTOMER_IX.
WITH params AS (
  SELECT :customer_id AS customer_id, :after_id AS after_id FROM dual
)
SELECT TO_CHAR(p.payee_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key1, '-' AS key2,
  JSON_OBJECT(
    'payee_id' VALUE TO_CHAR(p.payee_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'bank_code' VALUE p.bank_code,
    'bank_name' VALUE i.name,
    'account_no' VALUE p.account_no,
    'holder_name' VALUE p.holder_name,
    'alias' VALUE p.alias,
    'registered_at' VALUE TO_CHAR(p.registered_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'channel' VALUE p.channel,
    'session_id' VALUE p.session_id
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM payee p
LEFT JOIN institution i ON i.institution_code = p.bank_code
CROSS JOIN params
WHERE p.customer_id = params.customer_id AND p.deleted_at IS NULL
  AND (params.after_id IS NULL OR p.payee_id > params.after_id)
ORDER BY p.payee_id
FETCH FIRST :take ROWS ONLY
