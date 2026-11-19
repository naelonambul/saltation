SELECT a.account_id, a.account_no, linked.account_no, d.principal, d.close_type,
       TO_CHAR(d.closed_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM deposit_detail d
JOIN account a ON a.account_id = d.account_id
JOIN account linked ON linked.account_id = d.linked_account_id
JOIN transfer t ON t.request_id = :request_id
JOIN account_transaction l ON l.transfer_id = t.transfer_id AND l.account_id = a.account_id
