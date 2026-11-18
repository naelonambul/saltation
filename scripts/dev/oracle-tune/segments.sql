SET SERVEROUTPUT ON
DECLARE n NUMBER;
BEGIN
 FOR t IN (SELECT table_name FROM dba_tables WHERE owner='SALTATION_TUNE' ORDER BY table_name) LOOP
  EXECUTE IMMEDIATE 'SELECT COUNT(*) FROM saltation_tune."'||t.table_name||'"' INTO n;
  DBMS_OUTPUT.PUT_LINE(t.table_name||' rows='||n);
 END LOOP;
END;
/
SELECT segment_name,segment_type,tablespace_name,ROUND(bytes/POWER(1024,2),2) mb FROM dba_segments WHERE owner='SALTATION_TUNE' ORDER BY bytes DESC;
SELECT index_name,num_rows,leaf_blocks,distinct_keys,blevel,last_analyzed FROM dba_indexes WHERE owner='SALTATION_TUNE' AND index_name IN ('OUTBOX_PK','OUTBOX_CREATED_IX');
SELECT tablespace_name,ROUND(SUM(bytes)/POWER(1024,2),2) allocated_mb FROM dba_segments WHERE tablespace_name IN ('SALTATION_DATA','OUTBOX_TS') GROUP BY tablespace_name;
