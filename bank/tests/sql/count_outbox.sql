-- test.count_outbox: undelivered events left in the outbox.
SELECT COUNT(*) FROM outbox
