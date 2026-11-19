WITH subject AS (
    SELECT c.customer_id, c.phone, a.account_id, :opening_leg_id AS opening_leg_id,
        :home_device AS home_device, :other_device AS other_device,
        :start_at AS start_at, :end_at AS end_at
    FROM customer c
    JOIN account a ON a.customer_id = c.customer_id
    WHERE c.customer_no = :customer_no AND a.account_no = :account_no
),
activity (kind, activity_id, at_time) AS (
    SELECT 'login', l.request_id, l.occurred_at
    FROM subject f
    JOIN login_attempt l ON (l.customer_id = f.customer_id OR l.phone = f.phone
        OR l.device_id IN (f.home_device, f.other_device))
    WHERE l.occurred_at >= f.start_at AND l.occurred_at < f.end_at
    UNION ALL
    SELECT 'session', s.session_id, s.started_at
    FROM subject f
    JOIN auth_session s ON (s.customer_id = f.customer_id
        OR s.device_id IN (f.home_device, f.other_device))
    WHERE s.started_at >= f.start_at AND s.started_at < f.end_at
    UNION ALL
    SELECT 'request', r.request_id, r.occurred_at
    FROM subject f
    JOIN transaction_request r ON r.occurred_at >= f.start_at AND r.occurred_at < f.end_at
    LEFT JOIN auth_session s ON s.session_id = r.session_id
    WHERE s.customer_id = f.customer_id OR r.device_id IN (f.home_device, f.other_device)
    UNION ALL
    SELECT 'leg', COALESCE(l.request_id, TO_CHAR(l.tran_id)), l.occurred_at
    FROM subject f
    JOIN account_transaction l ON l.account_id = f.account_id
    WHERE l.occurred_at >= f.start_at AND l.occurred_at < f.end_at
        AND l.tran_id <> f.opening_leg_id
)
SELECT kind, activity_id,
    TO_CHAR(at_time AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
FROM activity
