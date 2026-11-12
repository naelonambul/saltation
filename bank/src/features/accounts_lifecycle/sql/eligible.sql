-- accounts_lifecycle.eligible: re-read activity after the caller locks the account.
-- Access path: ACCOUNT primary key.
SELECT COUNT(*) FROM account WHERE account_id = :account_id AND subtype = 'DEMAND' AND status = 'ACTIVE'
AND TRUNC(CAST(COALESCE(last_activity_at, opened_at) AT TIME ZONE '+09:00' AS TIMESTAMP)) <= :cutoff
