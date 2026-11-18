SELECT COUNT(*) FROM card c JOIN account a ON a.account_id = c.account_id WHERE a.account_no = :1
