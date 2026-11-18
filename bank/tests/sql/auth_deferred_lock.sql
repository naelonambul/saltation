-- test.auth_deferred_lock: hold the first deferred-touch lock.
-- Access path: ACCESS_TOKEN primary key.
SELECT token_hash FROM access_token WHERE token_hash = :hash FOR UPDATE WAIT 1
