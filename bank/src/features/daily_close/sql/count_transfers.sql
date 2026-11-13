-- daily_close.count_transfers: counts all malformed transfers including empty ones.
-- Access path: TRANSFER_OCCURRED_IX; ACCOUNT_TRANSACTION_TRANSFER_IX.
WITH shapes AS (
 SELECT t.transfer_id, t.kind, t.amount, COUNT(r.tran_id) n, COUNT(DISTINCT r.account_id) accounts, NVL(SUM(r.amount), 0) total,
 MIN(r.amount) minimum, MAX(r.amount) maximum,
 SUM(CASE WHEN ABS(r.amount) = t.amount THEN 0 ELSE 1 END) bad_amount,
 MAX(CASE WHEN r.counterparty_bank_code <> '998' AND EXISTS (SELECT 1 FROM institution i WHERE i.institution_code = r.counterparty_bank_code AND i.institution_type = 'BANK') THEN 1 ELSE 0 END) external_bank
 FROM transfer t LEFT JOIN account_transaction r ON r.transfer_id = t.transfer_id
 WHERE t.occurred_at >= :start_at AND t.occurred_at < :end_at
 GROUP BY t.transfer_id, t.kind, t.amount
), violations AS (
 SELECT transfer_id, kind FROM shapes WHERE bad_amount <> 0 OR CASE
 WHEN kind = 'transfer' AND ((n = 2 AND accounts = 2 AND total = 0 AND external_bank = 0) OR (n = 1 AND total = -amount AND external_bank = 1)) THEN 0
 WHEN kind IN ('term_open', 'term_close') AND n = 2 AND accounts = 2 AND total = 0 THEN 0
 WHEN kind = 'inbound' AND n = 1 AND total = amount THEN 0
 WHEN kind = 'loan_disburse' AND n = 2 AND accounts = 2 AND minimum = amount AND maximum = amount THEN 0
 WHEN kind = 'loan_repay' AND n = 2 AND accounts = 2 AND minimum = -amount AND maximum = -amount THEN 0
 ELSE 1 END = 1
)
SELECT COUNT(*) FROM violations
