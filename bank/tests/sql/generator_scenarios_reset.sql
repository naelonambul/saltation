SELECT TO_CHAR(r.issued_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
       TO_CHAR(r.used_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
       TO_CHAR(r.expires_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
       r.failed_count
FROM password_reset r
JOIN customer c ON c.customer_id = r.customer_id
WHERE c.customer_no = :customer_no
