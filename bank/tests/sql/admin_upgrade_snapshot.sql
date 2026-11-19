-- test.admin_upgrade_snapshot: verify representative existing records across the v21 schema upgrade.
SELECT (SELECT COUNT(*) FROM customer),
       (SELECT COUNT(*) FROM account_transaction),
       (SELECT COALESCE(SUM(amount), 0) FROM account_transaction),
       (SELECT COUNT(*) FROM access_token),
       (SELECT MAX(version) FROM schema_migration)
FROM dual
