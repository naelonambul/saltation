-- clock.set_time: persists the new business time after rollover.
-- Access path: SIM_CLOCK primary key (single row).
UPDATE sim_clock SET business_time = :business_time WHERE id = 'X'
