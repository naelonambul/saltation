-- db.create_migration_table: creates SCHEMA_MIGRATION if it does not exist.
-- Access path: none (DDL). ORA-00955 means the table already exists.
BEGIN
  EXECUTE IMMEDIATE 'CREATE TABLE schema_migration (
      version     NUMBER(6)       NOT NULL PRIMARY KEY,
      name        VARCHAR2(100)   NOT NULL,
      applied_at  TIMESTAMP(6) WITH TIME ZONE DEFAULT SYSTIMESTAMP NOT NULL)';
EXCEPTION WHEN OTHERS THEN
  IF SQLCODE != -955 THEN RAISE; END IF;
END;
