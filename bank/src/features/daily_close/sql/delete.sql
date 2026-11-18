-- daily_close.delete: replaces the date atomically.
-- Access path: date-leading DAILY_CLOSE_PK.
DELETE FROM daily_close WHERE business_date = TO_DATE(:business_date, 'YYYY-MM-DD')
