-- deposits.identity: generator-only ownership-scoped term and linked demand identity.
SELECT term.account_no, linked.account_no
FROM account term
JOIN deposit_detail d ON d.account_id = term.account_id
JOIN account linked ON linked.account_id = d.linked_account_id AND linked.customer_id = term.customer_id
WHERE term.account_id = :id AND term.customer_id = :customer
  AND term.subtype = 'TERM' AND linked.subtype = 'DEMAND'
