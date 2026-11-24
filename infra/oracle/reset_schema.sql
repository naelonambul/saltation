-- Test-only: recreates the empty SALTATION schema in ORCLPDB1.
-- Run as SYSDBA by scripts/dev/it-oracle.sh, which defines &pw first; never against a database with data to keep.
WHENEVER SQLERROR EXIT SQL.SQLCODE
ALTER SESSION SET CONTAINER = ORCLPDB1;
DECLARE
  v_dropped BOOLEAN := FALSE;
BEGIN
  FOR attempt IN 1..10 LOOP
    FOR s IN (SELECT sid, serial# FROM v$session WHERE username = 'SALTATION') LOOP
      BEGIN
        EXECUTE IMMEDIATE 'ALTER SYSTEM DISCONNECT SESSION ''' || s.sid || ',' || s.serial# || ''' IMMEDIATE';
      EXCEPTION WHEN OTHERS THEN
        IF SQLCODE NOT IN (-30, -31) THEN RAISE; END IF;
      END;
    END LOOP;
    BEGIN
      EXECUTE IMMEDIATE 'DROP USER saltation CASCADE';
      v_dropped := TRUE;
      EXIT;
    EXCEPTION WHEN OTHERS THEN
      IF SQLCODE = -1918 THEN
        v_dropped := TRUE;
        EXIT;
      ELSIF SQLCODE = -1940 AND attempt < 10 THEN
        DBMS_SESSION.SLEEP(0.5);
      ELSE
        RAISE;
      END IF;
    END;
  END LOOP;
  IF NOT v_dropped THEN
    RAISE_APPLICATION_ERROR(-20001, 'Could not drop SALTATION user after retries');
  END IF;
END;
/
CREATE USER saltation IDENTIFIED BY "&pw" DEFAULT TABLESPACE saltation_data
  QUOTA UNLIMITED ON saltation_data QUOTA UNLIMITED ON outbox_ts;
GRANT CREATE SESSION, CREATE TABLE, CREATE SEQUENCE, CREATE VIEW, CREATE PROCEDURE TO saltation;
GRANT SELECT ON SYS.V_$SQL TO saltation;
GRANT SELECT ON SYS.V_$SQL_PLAN TO saltation;
GRANT SELECT ON SYS.V_$SQL_PLAN_STATISTICS_ALL TO saltation;
GRANT SELECT ON SYS.V_$SESSION TO saltation;
EXIT
