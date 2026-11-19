-- Ownership via ACCOUNT_CUSTOMER_IX and CARD_ACCOUNT_IX.
WITH params AS (
  SELECT :customer_id AS customer_id, :after_id AS after_id FROM dual
)
SELECT TO_CHAR(c.card_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key1, '-' AS key2,
  JSON_OBJECT(
    'account_no' VALUE a.account_no,
    'card_id' VALUE TO_CHAR(c.card_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'card_no' VALUE c.card_no,
    'status' VALUE c.status,
    'issued_at' VALUE TO_CHAR(c.issued_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM account a
JOIN card c ON c.account_id = a.account_id
CROSS JOIN params
WHERE a.customer_id = params.customer_id
  AND (params.after_id IS NULL OR c.card_id > params.after_id)
ORDER BY c.card_id
FETCH FIRST :take ROWS ONLY
