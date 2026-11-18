-- daily_close.report: counts reconciliation results.
-- Access path: DAILY_CLOSE_PK date-leading range.
SELECT /*+ INDEX(c daily_close_pk) */ COUNT(*), NVL(SUM(CASE WHEN verified = 'N' THEN 1 ELSE 0 END), 0) FROM daily_close c WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
