-- accounts_lifecycle.candidates: active demand accounts inactive for 365 business dates.
-- Access path: ACCOUNT_DORMANCY_IX range scan.
SELECT account_id FROM account WHERE subtype = 'DEMAND' AND status = 'ACTIVE'
AND COALESCE(SYS_EXTRACT_UTC(last_activity_at), SYS_EXTRACT_UTC(opened_at)) < :cutoff_utc
ORDER BY account_id
