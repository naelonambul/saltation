-- test.atm_terminal: read exact seeded decimal coordinates.
-- Access path: ATM_TERMINAL primary key.
SELECT TO_CHAR(latitude, 'FM990.000000', 'NLS_NUMERIC_CHARACTERS=''.,'''),
       TO_CHAR(longitude, 'FM990.000000', 'NLS_NUMERIC_CHARACTERS=''.,''')
FROM atm_terminal WHERE terminal_id = :terminal_id
