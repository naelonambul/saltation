-- test.count_performance_indexes: indexes introduced by migration 11.
-- Access path: USER_INDEXES filtered by index name.
SELECT COUNT(*) FROM user_indexes
WHERE index_name IN ('ACCOUNT_TRANSACTION_TIME_IX', 'ACCOUNT_DORMANCY_IX', 'OUTBOX_CREATED_IX')
