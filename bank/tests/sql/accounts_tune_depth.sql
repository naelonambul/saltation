-- test.accounts_tune_depth: rolled-back shallow/deep index-depth witness.
INSERT INTO account_transaction (account_id, account_sequence, amount, balance_after,
    inout_type, tran_type, channel, request_id, business_date, occurred_at)
SELECT :account_id, LEVEL, 1, LEVEL, 'I', 'tune', 'MOBILE', :request_id,
    TRUNC(:at), :at
FROM dual CONNECT BY LEVEL <= :row_count
