SELECT device_id,
    TO_CHAR(first_seen AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    TO_CHAR(last_seen AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM device d
JOIN customer c ON c.customer_id = d.customer_id
WHERE c.customer_no = :customer_no
