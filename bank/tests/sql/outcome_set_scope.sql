-- test.outcome_set_scope: rejection persistence witness.
-- Access path: primary keys or full test snapshot.
UPDATE access_token SET scope = :scope WHERE token_hash = :hash
