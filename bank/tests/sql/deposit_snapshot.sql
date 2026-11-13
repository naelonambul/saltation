-- test.deposit_snapshot: reads balance and lifetime transfer-limit usage for an account.
-- Access path: FINTECH_ACCOUNT primary key and DAILY_ACCOUNT_TOTAL primary key prefix.
SELECT a.balance, NVL((SELECT SUM(transfer_out) FROM daily_account_total d WHERE d.account_id = a.account_id), 0)
FROM fintech_account f JOIN account a ON a.account_id = f.account_id WHERE f.fintech_use_num = :fin
