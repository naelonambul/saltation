-- test.admin_test_attempt: insert a trace intentionally unrelated to the target customer.
INSERT INTO login_attempt (phone, customer_id, outcome, request_id, occurred_at)
VALUES (:phone, (SELECT customer_id FROM customer WHERE customer_no = :different_customer),
        'UNKNOWN_PHONE', :request_id, :occurred_at)
