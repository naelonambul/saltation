-- payees.list_payees: the customer's active payees, newest first.
-- Access path: PAYEE_CUSTOMER_IX and INSTITUTION primary key.
SELECT p.payee_id, p.bank_code, i.name, p.account_no, p.holder_name, p.alias,
TO_CHAR(p.registered_at, 'YYYYMMDDHH24MISS')
FROM payee p JOIN institution i ON i.institution_code = p.bank_code WHERE p.customer_id = :customer_id AND p.deleted_at IS NULL ORDER BY p.registered_at DESC, p.payee_id DESC
