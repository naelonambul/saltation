-- test.reused_request_column: migration 15 column metadata.
-- Access path: Oracle user dictionary.
SELECT data_type, data_length, nullable FROM user_tab_columns WHERE table_name = 'TRANSACTION_REQUEST' AND column_name = 'REUSED_REQUEST_ID'
