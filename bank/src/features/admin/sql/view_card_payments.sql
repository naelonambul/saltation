-- admin.view_card_payments: page known-card decisions independently of ledger posting.
WITH params AS (
  SELECT :account_id account_id,
         FROM_TZ(CAST(:cursor_time AS TIMESTAMP), '+09:00') cursor_time,
         :cursor_request cursor_request
  FROM dual
)
SELECT TO_CHAR(r.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM') cursor_time,
       r.request_id cursor_request,
       JSON_OBJECT(
         'request_id' VALUE r.request_id,
         'card_no' VALUE c.card_no,
         'status' VALUE r.status,
         'rsp_code' VALUE r.rsp_code,
         'bank_rsp_code' VALUE r.bank_rsp_code,
         'occurred_at' VALUE TO_CHAR(r.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
         'merchant_name' VALUE ca.merchant_name,
         'mcc' VALUE ca.mcc,
         'country' VALUE ca.country,
         'currency' VALUE ca.currency,
         'original_amount' VALUE TO_CHAR(ca.original_amount, 'TM9'),
         'krw_amount' VALUE TO_CHAR(ca.krw_amount, 'TM9'),
         'entry_mode' VALUE ca.entry_mode,
         'latitude' VALUE TO_CHAR(ca.latitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,'''),
         'longitude' VALUE TO_CHAR(ca.longitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,''')
       ) item
FROM params p
JOIN card c ON c.account_id = p.account_id
JOIN card_authorization ca ON ca.card_id = c.card_id
JOIN transaction_request r ON r.request_id = ca.request_id
WHERE p.cursor_time IS NULL OR (
  r.occurred_at <= p.cursor_time
  AND (r.occurred_at < p.cursor_time
       OR (r.occurred_at = p.cursor_time AND r.request_id < p.cursor_request))
)
ORDER BY r.occurred_at DESC, r.request_id DESC
FETCH FIRST :take_count ROWS ONLY
