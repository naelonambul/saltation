-- outbox.select_backlog: undelivered count and the age of the oldest event in seconds.
-- Access path: OUTBOX_CREATED_IX full scan.
SELECT COUNT(*), NVL(EXTRACT(DAY FROM (SYSTIMESTAMP - MIN(created_at))) * 86400
                       + EXTRACT(HOUR FROM (SYSTIMESTAMP - MIN(created_at))) * 3600
                       + EXTRACT(MINUTE FROM (SYSTIMESTAMP - MIN(created_at))) * 60
                       + EXTRACT(SECOND FROM (SYSTIMESTAMP - MIN(created_at))), 0)
FROM outbox
