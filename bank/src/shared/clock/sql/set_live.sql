-- clock.set_live: permanently switches to wall time.
-- Access path: SIM_CLOCK primary key (single row).
UPDATE sim_clock SET clock_mode = 'LIVE', last_advanced_at = SYSTIMESTAMP WHERE id = 'X'
