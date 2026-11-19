-- admin.view_daily_close: page close results for a fixed date by numeric account ID.
WITH params AS (
  SELECT CAST(:business_date AS DATE) business_date, :account_no account_no,
         :after_account_id after_account_id
  FROM dual
)
SELECT TO_CHAR(d.business_date, 'YYYY-MM-DD') cursor_date,
       TO_CHAR(d.account_id, 'TM9') cursor_account_id,
       JSON_OBJECT(
         'account_no' VALUE a.account_no,
         'business_date' VALUE TO_CHAR(d.business_date, 'YYYY-MM-DD'),
         'opening_balance' VALUE TO_CHAR(d.opening_balance, 'TM9'),
         'closing_balance' VALUE TO_CHAR(d.closing_balance, 'TM9'),
         'verified' VALUE CASE d.verified WHEN 'Y' THEN 'true' ELSE 'false' END FORMAT JSON,
         'mismatch_detail' VALUE d.mismatch_detail
       ) item
FROM params p
JOIN daily_close d ON d.business_date = p.business_date
JOIN account a ON a.account_id = d.account_id
WHERE (p.account_no IS NULL OR a.account_no = p.account_no)
  AND (p.after_account_id IS NULL OR d.account_id > p.after_account_id)
ORDER BY d.account_id
FETCH FIRST :take_count ROWS ONLY
