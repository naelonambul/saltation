SELECT p.session_id,
       TO_CHAR(p.registered_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM payee p
JOIN customer c ON c.customer_id = p.customer_id
WHERE p.payee_id = :payee_id AND c.customer_no = :customer_no
  AND p.bank_code = '998' AND p.account_no = :account_no AND p.deleted_at IS NULL
