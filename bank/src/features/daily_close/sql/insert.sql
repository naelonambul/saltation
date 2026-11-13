-- daily_close.insert: closes every account open during the date.
-- Access path: ACCOUNT scan; ACCOUNT_TRANSACTION_OCCURRED_IX; DAILY_CLOSE_DATE_IX; ACCOUNT_STATUS_HISTORY_IX.
INSERT INTO daily_close (account_id, business_date, opening_balance, closing_balance, verified, mismatch_detail)
WITH bounds AS (SELECT TO_DATE(:business_date, 'YYYY-MM-DD') d, :start_at lo, :end_at hi FROM dual),
rows_of_date AS (
 SELECT r.account_id, SUM(r.amount) amount,
 MIN(r.balance_after - r.amount) KEEP (DENSE_RANK FIRST ORDER BY r.account_sequence) first_open,
 MAX(r.balance_after) KEEP (DENSE_RANK LAST ORDER BY r.account_sequence) last_balance,
 MAX(CASE WHEN r.request_id IS NULL AND r.channel <> 'SYSTEM' THEN 1 ELSE 0 END) missing_request,
 MAX(CASE WHEN r.transfer_id IS NULL THEN 1 ELSE 0 END) missing_transfer
 FROM account_transaction r CROSS JOIN bounds b WHERE r.occurred_at >= b.lo AND r.occurred_at < b.hi GROUP BY r.account_id
), previous AS (
 SELECT c.account_id, c.closing_balance balance
 FROM daily_close c CROSS JOIN bounds b WHERE c.business_date = b.d - 1
), balances AS (
 SELECT a.account_id, b.d, a.balance stored, r.last_balance, NVL(r.amount, 0) amount,
 NVL(r.missing_request, 0) missing_request, NVL(r.missing_transfer, 0) missing_transfer,
 CASE WHEN a.opened_at < b.lo AND p.account_id IS NULL AND EXISTS
 (SELECT 1 FROM daily_close c WHERE c.business_date < b.d) THEN 1 ELSE 0 END missing_previous,
 CASE WHEN a.opened_at >= b.lo THEN 0 ELSE COALESCE(p.balance, r.first_open, a.balance) END opening
 FROM account a CROSS JOIN bounds b LEFT JOIN rows_of_date r ON r.account_id = a.account_id
 LEFT JOIN previous p ON p.account_id = a.account_id
 WHERE a.opened_at < b.hi AND NOT EXISTS (
 SELECT 1 FROM account_status_history h WHERE h.account_id = a.account_id AND h.to_status = 'CLOSED' AND h.occurred_at < b.lo)
), checked AS (
 SELECT balances.*, opening + amount closing,
 CASE WHEN opening + amount <> stored OR (last_balance IS NOT NULL AND opening + amount <> last_balance)
 THEN 'balance: expected ' || TO_CHAR(opening + amount) || ' last_row ' || NVL(TO_CHAR(last_balance), 'none') || ' stored ' || TO_CHAR(stored)
 END ||
 CASE WHEN missing_previous = 1 THEN '; no previous close' END ||
 CASE WHEN missing_transfer = 1 THEN '; row without transfer' END ||
 CASE WHEN missing_request = 1 THEN '; non-SYSTEM row missing request_id' END detail FROM balances
)
SELECT account_id, d, opening, closing, CASE WHEN detail IS NULL THEN 'Y' ELSE 'N' END, SUBSTR(detail, 1, 400) FROM checked
