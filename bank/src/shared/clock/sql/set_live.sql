-- clock.set_live: permanently switches to wall time.
-- Access path: SIM_CLOCK primary key (single row).
UPDATE sim_clock SET clock_mode = 'LIVE' WHERE id = 'X'
