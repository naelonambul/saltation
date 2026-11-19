-- test.admin_upgrade_tokens: compare all prior token records without printing their credential digests.
SELECT token_hash, subject_type, subject_id, scope, session_id,
       expires_at, revoked_at
FROM access_token ORDER BY token_hash
