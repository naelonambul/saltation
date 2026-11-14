SELECT a.account_no FROM fintech_account f JOIN account a ON a.account_id = f.account_id WHERE f.fintech_use_num = :fin
