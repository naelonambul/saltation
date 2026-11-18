-- daily_close.mark_transfer_violations: append the minimum transfer detail for each account.
-- Access path: DAILY_CLOSE_PK.
UPDATE daily_close
SET verified = 'N', mismatch_detail = SUBSTR(CASE WHEN mismatch_detail IS NULL THEN :detail ELSE mismatch_detail || '; ' || :appended_detail END, 1, 400)
WHERE account_id = :account_id AND business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
