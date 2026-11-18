-- outbox.select_backlog: undelivered count and the age of the oldest event in seconds.
-- Access path: OUTBOX_PK count and index-only OUTBOX_CREATED_IX MIN/MAX.
SELECT (SELECT COUNT(*) FROM outbox),
       NVL(EXTRACT(DAY FROM (SYS_EXTRACT_UTC(SYSTIMESTAMP) - oldest.oldest_at)) * 86400
         + EXTRACT(HOUR FROM (SYS_EXTRACT_UTC(SYSTIMESTAMP) - oldest.oldest_at)) * 3600
         + EXTRACT(MINUTE FROM (SYS_EXTRACT_UTC(SYSTIMESTAMP) - oldest.oldest_at)) * 60
         + EXTRACT(SECOND FROM (SYS_EXTRACT_UTC(SYSTIMESTAMP) - oldest.oldest_at)), 0)
FROM (SELECT /*+ INDEX(o outbox_created_ix) */ MIN(SYS_EXTRACT_UTC(o.created_at)) oldest_at
      FROM outbox o) oldest
