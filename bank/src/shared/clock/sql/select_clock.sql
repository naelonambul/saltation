-- clock.select: reads persisted business time and rollover progress.
-- Access path: SIM_CLOCK primary key (single row).
SELECT clock_mode, business_time, TO_CHAR(rolled_through, 'YYYY-MM-DD') FROM sim_clock WHERE id = 'X'
