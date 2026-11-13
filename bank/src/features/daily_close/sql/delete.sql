-- daily_close.delete: replaces the date atomically.
-- Access path: DAILY_CLOSE_DATE_IX.
DELETE FROM daily_close WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
