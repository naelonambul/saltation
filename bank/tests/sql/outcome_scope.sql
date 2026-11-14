-- test.outcome_scope: rejection persistence witness.
-- Access path: primary keys or full test snapshot.
SELECT scope FROM access_token WHERE token_hash = :hash
