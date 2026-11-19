SELECT JSON_OBJECT(
    'customer_no' VALUE c.customer_no,
    'outcome' VALUE l.outcome,
    'device_id' VALUE l.device_id,
    'ip' VALUE l.ip,
    'region' VALUE l.region,
    'occurred_at' VALUE TO_CHAR(l.occurred_at AT TIME ZONE '+09:00', 'YYYY-MM-DD"T"HH24:MI:SS.FF6TZH:TZM'),
    'session_id' VALUE s.session_id,
    'session_listener' VALUE s.listener,
    'session_human' VALUE s.human,
    'session_device' VALUE s.device_id,
    'session_ip' VALUE s.ip,
    'session_region' VALUE s.region
)
FROM login_attempt l
LEFT JOIN customer c ON c.customer_id = l.customer_id
LEFT JOIN auth_session s ON s.customer_id = l.customer_id AND s.started_at = l.occurred_at
WHERE l.request_id = :request_id
