-- deposits.candidates: finds outstanding matured deposits.
-- Access path: DEPOSIT_DETAIL_MATURITY_IX date range.
SELECT account_id, linked_account_id, principal FROM deposit_detail WHERE maturity_date <= :business_date AND closed_at IS NULL ORDER BY account_id
