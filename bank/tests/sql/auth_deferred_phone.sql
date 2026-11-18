-- test.auth_deferred_phone: synthetic customer's phone for the reset API.
-- Access path: ACCESS_TOKEN and CUSTOMER primary keys.
SELECT c.phone FROM access_token t JOIN customer c ON c.customer_id = t.subject_id
WHERE t.token_hash = :hash AND t.subject_type = 'CUSTOMER'
