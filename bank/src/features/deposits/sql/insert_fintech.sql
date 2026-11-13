-- deposits.insert_fintech: registers the term fintech number.
-- Access path: FINTECH_ACCOUNT primary key and FINTECH_ACCOUNT_UK.
INSERT INTO fintech_account (fintech_use_num, account_id, user_seq_no) VALUES (:fin, :id, :user_seq)
