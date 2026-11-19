-- test.admin_token_scope: build an otherwise valid token with deliberately wrong scope.
UPDATE access_token SET scope = :new_scope WHERE token_hash = :token_hash
