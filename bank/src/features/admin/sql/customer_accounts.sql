-- Ownership via ACCOUNT_CUSTOMER_IX, with product terms by primary key.
WITH params AS (
  SELECT :customer_id AS customer_id, :after_id AS after_id FROM dual
)
SELECT TO_CHAR(a.account_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key1, '-' AS key2,
  JSON_OBJECT(
    'account_no' VALUE a.account_no,
    'subtype' VALUE a.subtype,
    'status' VALUE a.status,
    'balance' VALUE TO_CHAR(a.balance, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'opened_at' VALUE TO_CHAR(a.opened_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'last_activity_at' VALUE TO_CHAR(a.last_activity_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'product_id' VALUE TO_CHAR(p.product_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'product_name' VALUE p.name,
    'terms_id' VALUE TO_CHAR(t.terms_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'terms_version' VALUE t.version,
    'term_months' VALUE t.term_months,
    'min_amount' VALUE TO_CHAR(t.min_amount, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'max_amount' VALUE TO_CHAR(t.max_amount, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'early_close_ok' VALUE t.early_close_ok
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM account a
JOIN product_terms t ON t.terms_id = a.terms_id
JOIN product p ON p.product_id = t.product_id
CROSS JOIN params
WHERE a.customer_id = params.customer_id
  AND (params.after_id IS NULL OR a.account_id > params.after_id)
ORDER BY a.account_id
FETCH FIRST :take ROWS ONLY
