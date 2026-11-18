-- test.tune_migration_snapshot: persisted contents around forward index migrations.
SELECT (SELECT COUNT(*) FROM customer), (SELECT COUNT(*) FROM account_transaction),
       (SELECT NVL(SUM(amount), 0) FROM account_transaction),
       (SELECT COUNT(*) FROM password_reset), (SELECT COUNT(*) FROM payee),
       (SELECT COUNT(*) FROM daily_close),
       (SELECT NVL(SUM(closing_balance), 0) FROM daily_close),
       (SELECT COUNT(*) FROM daily_close WHERE verified = 'N')
FROM dual
