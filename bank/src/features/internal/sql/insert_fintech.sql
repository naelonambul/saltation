-- internal.insert_fintech: binds an Open Banking fintech_use_num to an account.
-- Access path: FINTECH_ACCOUNT primary key.
INSERT INTO fintech_account (fintech_use_num, account_id, user_seq_no) VALUES (:fintech_use_num, :account_id, :user_seq_no)
