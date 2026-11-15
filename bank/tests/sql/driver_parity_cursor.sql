SELECT sql_id, child_number FROM v$sql WHERE sql_text LIKE 'SELECT /* driver_parity_lookup */ account_id%' AND parsing_schema_name = USER ORDER BY last_active_time DESC
