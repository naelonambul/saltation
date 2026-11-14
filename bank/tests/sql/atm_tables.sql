-- test.atm_tables
SELECT table_name FROM user_tables
WHERE table_name IN ('ATM_TERMINAL', 'ATM_CREDENTIAL', 'ATM_SESSION')
ORDER BY table_name
