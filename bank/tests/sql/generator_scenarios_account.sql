SELECT c.customer_no, a.subtype, a.status, a.balance, a.terms_id,
       TO_CHAR(a.opened_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM account a
JOIN customer c ON c.customer_id = a.customer_id
WHERE a.account_no = :account_no
