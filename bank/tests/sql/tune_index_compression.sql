-- test.tune_index_compression: required date-prefix compression and uniqueness.
SELECT compression, prefix_length, uniqueness FROM user_indexes WHERE index_name = 'DAILY_CLOSE_PK'
