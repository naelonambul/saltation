SELECT clock_mode,
       TO_CHAR(business_time AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
       TO_CHAR(last_advanced_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM sim_clock
WHERE id = 'X'
