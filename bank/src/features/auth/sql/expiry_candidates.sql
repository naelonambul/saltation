SELECT s.session_id, c.customer_no, s.device_id, s.last_seen
FROM auth_session s JOIN customer c ON c.customer_id = s.customer_id
WHERE CASE WHEN s.ended_at IS NULL THEN s.last_seen END <= :cutoff
ORDER BY CASE WHEN s.ended_at IS NULL THEN s.last_seen END, s.session_id
FETCH FIRST :batch_size ROWS ONLY
