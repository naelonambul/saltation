-- payees.select_payee: one active payee of the customer.
-- Access path: PAYEE primary key and INSTITUTION primary key.
SELECT p.payee_id, p.bank_code, i.name, p.account_no, p.holder_name, p.alias,
TO_CHAR(p.registered_at, 'YYYYMMDDHH24MISS')
FROM payee p JOIN institution i ON i.institution_code = p.bank_code WHERE p.payee_id = :payee_id AND p.customer_id = :customer_id AND p.deleted_at IS NULL
