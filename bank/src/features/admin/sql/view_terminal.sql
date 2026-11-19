-- admin.view_terminal: resolve the terminal recorded on the ATM ledger row.
SELECT JSON_OBJECT(
         'terminal_id' VALUE t.terminal_id,
         'region' VALUE t.region,
         'location' VALUE t.location,
         'latitude' VALUE TO_CHAR(t.latitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,'''),
         'longitude' VALUE TO_CHAR(t.longitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,''')
       ) item
FROM account_transaction tx
JOIN atm_terminal t ON t.terminal_id = tx.terminal_id
WHERE tx.tran_id = :tran_id
  AND tx.channel = 'ATM'
