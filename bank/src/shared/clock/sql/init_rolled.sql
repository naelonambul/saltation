-- clock.init_rolled: initializes the last completed business date.
-- Access path: SIM_CLOCK primary key (single row).
UPDATE sim_clock SET rolled_through = TRUNC(CAST((CASE WHEN clock_mode = 'LIVE' THEN SYSTIMESTAMP ELSE business_time END) AT TIME ZONE '+09:00' AS DATE)) - 1 WHERE id = 'X' AND rolled_through IS NULL
