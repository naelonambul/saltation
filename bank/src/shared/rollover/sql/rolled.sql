-- rollover.rolled: records the last fully completed business date.
-- Access path: SIM_CLOCK primary key.
UPDATE sim_clock SET rolled_through = TO_DATE(:business_date, 'YYYY-MM-DD') WHERE id = 'X'
