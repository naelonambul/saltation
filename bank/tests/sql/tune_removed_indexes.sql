-- test.tune_removed_indexes: only the audited redundant indexes are gone.
SELECT COUNT(*) FROM user_indexes WHERE index_name IN ('TRANSACTION_REQUEST_BTID_IX', 'DAILY_CLOSE_DATE_IX')
