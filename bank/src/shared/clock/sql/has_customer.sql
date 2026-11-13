-- clock.has_customer: checks whether backward movement is forbidden.
-- Access path: CUSTOMER primary key.
SELECT COUNT(*) FROM customer WHERE ROWNUM = 1
