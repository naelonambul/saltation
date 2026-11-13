-- clock.select: reads persisted business time and rollover progress.
-- Access path: SIM_CLOCK primary key (single row).
SELECT clock_mode, business_time, TO_CHAR(rolled_through, 'YYYY-MM-DD'),
       (SELECT COUNT(*) FROM rollover_run r WHERE r.business_date > sim_clock.rolled_through)
FROM sim_clock WHERE id = 'X'
