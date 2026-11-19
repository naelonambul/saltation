-- test.admin_schema_objects: check the analyst table and customer name index.
SELECT (SELECT COUNT(*) FROM user_tables WHERE table_name = 'ANALYST'),
       (SELECT COUNT(*) FROM user_ind_columns WHERE index_name = 'CUSTOMER_NAME_ID_IX'
          AND ((column_position = 1 AND column_name = 'NAME')
            OR (column_position = 2 AND column_name = 'CUSTOMER_ID')))
FROM dual
