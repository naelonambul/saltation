-- test.admin_test_request_time: set an exact business timestamp on a card query fixture.
UPDATE transaction_request SET occurred_at = :occurred_at WHERE request_id = :request_id
