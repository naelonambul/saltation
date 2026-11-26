SELECT token_hash FROM access_token WHERE token_hash = :token_hash FOR UPDATE WAIT 1
