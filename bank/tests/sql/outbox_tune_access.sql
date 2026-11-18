-- test.outbox_tune_access: access to the oldest event in the production query.
SELECT operation, options FROM v$sql_plan
WHERE sql_id = :sql_id AND child_number = :child
  AND object_name = 'OUTBOX_CREATED_IX'
