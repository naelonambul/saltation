-- test.outbox_tablespace: where the outbox table and its primary key index live.
SELECT t.tablespace_name || '/' || i.tablespace_name
FROM user_tables t JOIN user_indexes i ON i.table_name = t.table_name
WHERE t.table_name = 'OUTBOX' AND i.index_name = 'OUTBOX_PK'
