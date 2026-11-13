-- test.payee_metadata: compare registration metadata with the authenticated token.
-- Access path: PAYEE primary key and ACCESS_TOKEN primary key.
SELECT p.channel, p.session_id, t.session_id FROM payee p
JOIN access_token t ON t.subject_id = p.customer_id
WHERE p.payee_id = :payee_id AND t.token_hash = :token_hash
