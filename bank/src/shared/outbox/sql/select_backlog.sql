-- outbox.select_backlog: undelivered count and the age of the oldest event in seconds.
-- Access path: OUTBOX_PK full scan (the table holds only undelivered events).
SELECT COUNT(*), NVL(MAX(EXTRACT(DAY FROM (SYSTIMESTAMP - created_at)) * 86400
                       + EXTRACT(HOUR FROM (SYSTIMESTAMP - created_at)) * 3600
                       + EXTRACT(MINUTE FROM (SYSTIMESTAMP - created_at)) * 60
                       + EXTRACT(SECOND FROM (SYSTIMESTAMP - created_at))), 0)
FROM outbox
