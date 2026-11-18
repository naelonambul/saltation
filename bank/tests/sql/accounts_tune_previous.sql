-- test.accounts_tune_previous: the production cursor executed on this lease.
SELECT prev_sql_id, prev_child_number FROM v$session
WHERE sid = SYS_CONTEXT('USERENV', 'SID')
