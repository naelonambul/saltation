-- accounts_lifecycle.candidates: active demand accounts inactive for 365 business dates.
-- Access path: ACCOUNT scan, ordered by primary key.
SELECT account_id FROM account WHERE subtype = 'DEMAND' AND status = 'ACTIVE'
AND TRUNC(CAST(COALESCE(last_activity_at, opened_at) AT TIME ZONE '+09:00' AS TIMESTAMP)) <= :cutoff
ORDER BY account_id
