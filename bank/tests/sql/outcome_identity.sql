SELECT c.customer_no, t.session_id FROM access_token t JOIN customer c ON c.customer_id = t.subject_id WHERE t.token_hash = :hash AND t.subject_type = 'CUSTOMER'
