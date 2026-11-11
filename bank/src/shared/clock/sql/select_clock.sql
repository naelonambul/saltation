-- clock.select: the persisted clock mode and business time.
-- Access path: SIM_CLOCK primary key (single row).
SELECT clock_mode, business_time FROM sim_clock WHERE id = 'X'
