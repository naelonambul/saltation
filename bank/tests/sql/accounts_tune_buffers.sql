-- test.accounts_tune_buffers: one real production child cursor's logical reads.
SELECT buffer_gets, executions FROM v$sql WHERE sql_id = :sql_id AND child_number = :child
