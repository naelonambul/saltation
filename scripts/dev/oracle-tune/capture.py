#!/usr/bin/env python3
"""Emit read-only SYSDBA capture SQL, matching production SQL by its file text."""
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
entry = sys.argv[2]
mode = sys.argv[3]
print('SET SERVEROUTPUT ON SIZE UNLIMITED\nSET LONG 1000000 LONGCHUNKSIZE 1000000\nSET PAGES 50000 LINES 240')
if mode == 'ash':
    workload, concurrency = entry.rsplit('-', 1)
    print(f"PROMPT ASH windows for {entry}; samples are not wait durations; an empty profile is possible for short windows")
    print(f"SELECT * FROM saltation_tune.tune_windows WHERE workload='{workload}' AND concurrency={int(concurrency)} ORDER BY run;")
    bounds = f"""FROM v$active_session_history h JOIN saltation_tune.tune_windows w
ON h.sample_time >= CAST(w.started_at AS TIMESTAMP) AND h.sample_time <= CAST(w.ended_at AS TIMESTAMP)
WHERE h.user_id=(SELECT user_id FROM dba_users WHERE username='SALTATION_TUNE')
AND w.workload='{workload}' AND w.concurrency={int(concurrency)}"""
    for title, query in [
        ('Top events per window', f"SELECT w.run,h.session_state,NVL(h.event,'ON CPU') event,COUNT(*) samples {bounds} GROUP BY w.run,h.session_state,h.event ORDER BY w.run,samples DESC"),
        ('Top SQL IDs per window (text follows)', f"SELECT w.run,h.sql_id,COUNT(*) samples {bounds} GROUP BY w.run,h.sql_id ORDER BY w.run,samples DESC"),
        ('SQL text', f"SELECT s.sql_id,s.sql_fulltext FROM v$sqlarea s WHERE s.sql_id IN (SELECT h.sql_id {bounds})"),
        ('Blocking sessions per window', f"SELECT w.run,h.blocking_session_status,h.blocking_inst_id,h.blocking_session,COUNT(*) samples {bounds} GROUP BY w.run,h.blocking_session_status,h.blocking_inst_id,h.blocking_session ORDER BY w.run,samples DESC"),
        ('enq: TX breakdown per window', f"SELECT w.run,h.event,h.p1text,h.p1,h.p2text,h.p2,h.p3text,h.p3,MOD(h.p1,65536) lock_mode,COUNT(*) samples {bounds} AND h.event LIKE 'enq: TX%' GROUP BY w.run,h.event,h.p1text,h.p1,h.p2text,h.p2,h.p3text,h.p3 ORDER BY w.run,samples DESC"),
    ]:
        print(f'PROMPT {title}\n{query};')
else:
    # Exact complete SQL text, including comments: this is what SqlStmt sends to Oracle.
    # Capture every executed production statement, a superset of findings 1--8.
    print('PROMPT Cumulative cached production cursors after this workload, including setup/warm-up; not per-window execution deltas')
    print("DECLARE n NUMBER; BEGIN")
    for path in sorted((root / 'bank/src').glob('**/sql/*.sql')):
        sql = path.read_text().strip()
        name = str(path.relative_to(root / 'bank/src'))
        literal = sql.replace("'", "''")
        print(f"DBMS_OUTPUT.PUT_LINE('FILE {name}'); n:=0;")
        print(f"FOR c IN (SELECT sql_id,child_number,executions,plan_hash_value,last_active_time FROM v$sql WHERE parsing_schema_name='SALTATION_TUNE' AND executions>0 AND DBMS_LOB.COMPARE(REGEXP_REPLACE(sql_fulltext,'[[:space:]]+$',''),TO_CLOB('{literal}'))=0) LOOP")
        print("n:=n+1; DBMS_OUTPUT.PUT_LINE('SQL_ID '||c.sql_id||' child '||c.child_number||' executions '||c.executions||' plan '||c.plan_hash_value);")
        print("FOR p IN (SELECT plan_table_output line FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(c.sql_id,c.child_number,'ALLSTATS LAST +PEEKED_BINDS'))) LOOP DBMS_OUTPUT.PUT_LINE(p.line); END LOOP; END LOOP;")
        print("IF n=0 THEN DBMS_OUTPUT.PUT_LINE('NOT EXECUTED / NOT IN SHARED POOL'); END IF;")
    print('END;\n/')
