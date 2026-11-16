SELECT plan_table_output FROM TABLE(dbms_xplan.display_cursor(:sql_id, :child, 'BASIC +PREDICATE'))
