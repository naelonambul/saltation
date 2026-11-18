-- test.ledger_tune_access: runtime predicates from the actual DATE-bound merge.
SELECT access_predicates, filter_predicates FROM v$sql_plan
WHERE sql_id = :sql_id AND child_number = :child
  AND object_name = 'DAILY_ACCOUNT_TOTAL_PK'
