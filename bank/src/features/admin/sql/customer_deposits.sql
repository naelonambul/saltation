-- Ownership via ACCOUNT_CUSTOMER_IX, with deposit detail by primary key.
WITH params AS (
  SELECT :customer_id AS customer_id, :after_id AS after_id FROM dual
)
SELECT TO_CHAR(a.account_id, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS key1, '-' AS key2,
  JSON_OBJECT(
    'account_no' VALUE a.account_no,
    'linked_account_no' VALUE linked.account_no,
    'product_name' VALUE p.name,
    'status' VALUE a.status,
    'balance' VALUE TO_CHAR(a.balance, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'principal' VALUE TO_CHAR(d.principal, 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,'''),
    'opened_at' VALUE TO_CHAR(a.opened_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'maturity_date' VALUE TO_CHAR(d.maturity_date, 'YYYY-MM-DD'),
    'closed_at' VALUE TO_CHAR(d.closed_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'close_type' VALUE d.close_type
    NULL ON NULL RETURNING VARCHAR2(4000)
  ) AS item_json
FROM account a
JOIN deposit_detail d ON d.account_id = a.account_id
JOIN account linked ON linked.account_id = d.linked_account_id
JOIN product_terms t ON t.terms_id = a.terms_id
JOIN product p ON p.product_id = t.product_id
CROSS JOIN params
WHERE a.customer_id = params.customer_id AND a.subtype = 'TERM'
  AND (params.after_id IS NULL OR a.account_id > params.after_id)
ORDER BY a.account_id
FETCH FIRST :take ROWS ONLY
