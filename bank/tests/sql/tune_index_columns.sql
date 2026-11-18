-- test.tune_index_columns: key order and reset lookup projection.
SELECT column_name FROM user_ind_columns WHERE index_name = :index_name ORDER BY column_position
