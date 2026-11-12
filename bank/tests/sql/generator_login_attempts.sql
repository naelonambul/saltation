-- test.generator_login_attempts: login attempts with a customer since a time.
SELECT a.request_id, c.customer_no FROM login_attempt a JOIN customer c ON c.customer_id = a.customer_id
WHERE a.recorded_at >= :since AND a.request_id IS NOT NULL
