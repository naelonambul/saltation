-- atm.terminal: read terminal event location.
-- Access path: ATM_TERMINAL primary key.
SELECT terminal_id, region, TO_CHAR(latitude, 'FM990.000000'), TO_CHAR(longitude, 'FM990.000000') FROM atm_terminal WHERE terminal_id = :terminal
