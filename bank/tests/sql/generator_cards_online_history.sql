WITH holder AS (SELECT customer_id FROM customer WHERE customer_no = :customer_no)
SELECT
    (SELECT COUNT(*) FROM login_attempt l JOIN holder h ON h.customer_id = l.customer_id
     WHERE l.region <> 'KR' AND l.region NOT LIKE 'KR-%')
    +
    (SELECT COUNT(*) FROM auth_session s JOIN holder h ON h.customer_id = s.customer_id
     WHERE s.region <> 'KR' AND s.region NOT LIKE 'KR-%')
FROM dual
