-- test.admin_test_name: set a distinctive name on a seeded test customer.
UPDATE customer SET name = :test_name WHERE customer_no = :customer_no
