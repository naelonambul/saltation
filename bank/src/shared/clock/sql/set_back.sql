-- clock.set_back: resets an empty bank's business time and rollover progress.
-- Access path: SIM_CLOCK primary key (single row).
UPDATE sim_clock SET business_time = :business_time, rolled_through = TO_DATE(:business_date, 'YYYY-MM-DD') WHERE id = 'X'
