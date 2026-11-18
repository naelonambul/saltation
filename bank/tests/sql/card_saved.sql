SELECT a.account_no, c.status FROM card c JOIN account a ON a.account_id = c.account_id WHERE c.card_no = :1
