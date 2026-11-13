-- accounts_lifecycle.eligible: re-read activity after the caller locks the account.
-- Access path: ACCOUNT_DORMANCY_IX range scan.
SELECT COUNT(*) FROM account WHERE account_id = :account_id AND subtype = 'DEMAND' AND status = 'ACTIVE'
AND COALESCE(SYS_EXTRACT_UTC(last_activity_at), SYS_EXTRACT_UTC(opened_at)) < :cutoff_utc
