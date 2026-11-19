SELECT JSON_OBJECT(
    'account_no' VALUE s.account_no,
    'session_id' VALUE s.session_id,
    'terminal_id' VALUE s.terminal_id,
    'region' VALUE t.region,
    'latitude' VALUE TO_CHAR(t.latitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,'''),
    'longitude' VALUE TO_CHAR(t.longitude, 'FM990D000000', 'NLS_NUMERIC_CHARACTERS=''.,'''),
    'started_at' VALUE TO_CHAR(s.started_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
)
FROM atm_session s
JOIN atm_terminal t ON t.terminal_id = s.terminal_id
WHERE s.request_id = :id
