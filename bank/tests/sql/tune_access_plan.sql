-- test.tune_access_plan: actual index access chosen for a production cursor.
SELECT operation, options, object_name FROM v$sql_plan
WHERE sql_id = :sql_id AND child_number = :child AND operation = 'INDEX'
