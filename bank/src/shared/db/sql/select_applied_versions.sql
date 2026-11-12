-- db.select_applied_versions: versions already applied.
-- Access path: SCHEMA_MIGRATION primary key, full scan (tiny table).
SELECT version FROM schema_migration ORDER BY version
