-- clock.init: creates the clock row in BACKFILL mode if missing.
-- Access path: SIM_CLOCK primary key (single row).
MERGE INTO sim_clock c
USING (SELECT 'X' AS id FROM dual) s ON (c.id = s.id)
WHEN NOT MATCHED THEN INSERT (id, clock_mode, business_time) VALUES ('X', 'BACKFILL', SYSTIMESTAMP)
