-- clock.init: creates the clock row in LIVE mode if it does not exist.
-- Access path: SIM_CLOCK primary key.
MERGE INTO sim_clock c
USING (SELECT 'X' AS id FROM dual) s ON (c.id = s.id)
WHEN NOT MATCHED THEN INSERT (id, clock_mode, business_time) VALUES ('X', 'LIVE', SYSTIMESTAMP)
