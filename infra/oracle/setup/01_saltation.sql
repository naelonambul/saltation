-- Runs once, as SYSDBA in the CDB root, right after the database is created
-- (the image runs every *.sql in /opt/oracle/scripts/setup).
-- Applies the spec's storage settings (N-7, F-36a) and creates the schema owner.
WHENEVER SQLERROR EXIT SQL.SQLCODE

-- Redo: three groups of 1 GB, replacing the image defaults.
ALTER DATABASE ADD LOGFILE GROUP 11 ('/opt/oracle/oradata/ORCLCDB/redo11.log') SIZE 1G;
ALTER DATABASE ADD LOGFILE GROUP 12 ('/opt/oracle/oradata/ORCLCDB/redo12.log') SIZE 1G;
ALTER DATABASE ADD LOGFILE GROUP 13 ('/opt/oracle/oradata/ORCLCDB/redo13.log') SIZE 1G;
BEGIN
  FOR g IN (SELECT group# FROM v$log WHERE group# < 11) LOOP
    LOOP
      BEGIN
        EXECUTE IMMEDIATE 'ALTER SYSTEM SWITCH LOGFILE';
        EXECUTE IMMEDIATE 'ALTER SYSTEM CHECKPOINT';
        EXECUTE IMMEDIATE 'ALTER DATABASE DROP LOGFILE GROUP ' || g.group#;
        EXIT;
      EXCEPTION WHEN OTHERS THEN
        IF SQLCODE IN (-1623, -1624) THEN DBMS_SESSION.SLEEP(1); ELSE RAISE; END IF;
      END;
    END LOOP;
  END LOOP;
END;
/

-- Undo: capped at 4 GB.
BEGIN
  FOR f IN (SELECT file_name FROM dba_data_files WHERE tablespace_name LIKE 'UNDOTBS%') LOOP
    EXECUTE IMMEDIATE 'ALTER DATABASE DATAFILE ''' || f.file_name || ''' AUTOEXTEND ON MAXSIZE 4G';
  END LOOP;
END;
/

-- PGA hard limit (N-7); 2 GB is the minimum Oracle 19c accepts.
ALTER SYSTEM SET pga_aggregate_limit = 2048M SCOPE = BOTH;

ALTER SESSION SET CONTAINER = ORCLPDB1;

CREATE TABLESPACE saltation_data DATAFILE '/opt/oracle/oradata/ORCLCDB/ORCLPDB1/saltation_data01.dbf'
  SIZE 1G AUTOEXTEND ON NEXT 512M MAXSIZE UNLIMITED;
CREATE TABLESPACE outbox_ts DATAFILE '/opt/oracle/oradata/ORCLCDB/ORCLPDB1/outbox01.dbf'
  SIZE 256M AUTOEXTEND ON NEXT 128M MAXSIZE 4G;

-- Schema owner; the password is replaced at runtime from SALTATION_DB_PASSWORD
-- by 02_password.sh.
CREATE USER saltation IDENTIFIED BY "ChangeMe_0" DEFAULT TABLESPACE saltation_data
  QUOTA UNLIMITED ON saltation_data QUOTA UNLIMITED ON outbox_ts;
GRANT CREATE SESSION, CREATE TABLE, CREATE SEQUENCE, CREATE VIEW, CREATE PROCEDURE TO saltation;
