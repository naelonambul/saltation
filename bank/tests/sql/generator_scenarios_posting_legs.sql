SELECT a.account_no, l.amount
FROM account_transaction l
JOIN account a ON a.account_id = l.account_id
WHERE l.request_id = :request_id OR l.transfer_id = :transfer_id
