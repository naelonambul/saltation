SELECT h.from_status, h.to_status, h.reason,
       TO_CHAR(h.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM account_status_history h
JOIN account a ON a.account_id = h.account_id
WHERE a.account_no = :account_no
ORDER BY h.occurred_at
