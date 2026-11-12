-- clock.forget_rollover: forgets jobs after an empty bank rewinds.
-- Access path: ROLLOVER_RUN primary key date range.
DELETE FROM rollover_run WHERE business_date > TO_DATE(:business_date, 'YYYY-MM-DD')
