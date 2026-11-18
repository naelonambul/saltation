-- test.accounts_tune_plan: actual adapter bind types, not EXPLAIN string binds.
SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(:sql_id, :child, 'BASIC +PREDICATE'))
