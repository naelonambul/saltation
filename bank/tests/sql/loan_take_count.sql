-- test.loan_take_count: counts loans and disbursements linked to a dedicated demand account.
-- Access path: FINTECH_ACCOUNT primary key, LOAN_DETAIL linked account and ACCOUNT_TRANSACTION account lookup.
SELECT COUNT(*), (SELECT COUNT(*) FROM transfer t WHERE t.kind = 'loan_disburse' AND t.transfer_id IN (SELECT r.transfer_id FROM account_transaction r WHERE r.account_id = f.account_id))
FROM fintech_account f JOIN loan_detail d ON d.linked_account_id = f.account_id WHERE f.fintech_use_num = :fin GROUP BY f.account_id
