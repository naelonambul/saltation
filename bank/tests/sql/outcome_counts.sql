-- test.outcome_counts: rejection persistence witness.
-- Access path: primary keys or full test snapshot.
SELECT (SELECT COUNT(*) FROM transaction_request), (SELECT COUNT(*) FROM transfer) FROM dual
