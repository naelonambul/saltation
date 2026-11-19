SELECT JSON_OBJECT(
    'customer_no' VALUE c.customer_no,
    'outcome' VALUE l.outcome,
    'region' VALUE l.region,
    'session_id' VALUE s.session_id,
    'occurred_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM')
)
FROM login_attempt l
JOIN customer c ON c.customer_id = l.customer_id
JOIN auth_session s ON s.customer_id = l.customer_id
    AND s.started_at = l.occurred_at AND s.region = l.region
    AND s.listener = 'GENERATOR' AND s.human = 'N'
WHERE l.request_id = :id
