-- internal.insert_atm_terminal: seed a terminal with exact decimal coordinates.
-- Access path: ATM_TERMINAL primary key.
INSERT INTO atm_terminal (terminal_id, region, location, latitude, longitude)
VALUES (:terminal_id, :region, :location,
        TO_NUMBER(:latitude, '999.999999', 'NLS_NUMERIC_CHARACTERS=''.,'''),
        TO_NUMBER(:longitude, '999.999999', 'NLS_NUMERIC_CHARACTERS=''.,'''))
