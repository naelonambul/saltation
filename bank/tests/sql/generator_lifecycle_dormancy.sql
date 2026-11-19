SELECT TO_CHAR(a.opened_at AT TIME ZONE '+09:00', 'YYYY-MM-DD'),
       TO_CHAR(h.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD'),
       (SELECT COUNT(*)
        FROM account_transaction l
        WHERE l.account_id = a.account_id AND l.occurred_at < h.occurred_at)
FROM account a
JOIN account_status_history h ON h.account_id = a.account_id
WHERE a.account_no = :account_no AND h.to_status = 'DORMANT'
