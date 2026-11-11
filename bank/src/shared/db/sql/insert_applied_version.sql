-- db.insert_applied_version: records one applied migration.
-- Access path: SCHEMA_MIGRATION primary key.
INSERT INTO schema_migration (version, name) VALUES (:version, :name)
