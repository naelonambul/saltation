-- auth.deferred_touch: slide activity with completion, token before session.
-- Access path: ACCESS_TOKEN and AUTH_SESSION primary keys; bounded row locks.
DECLARE
    v_hash access_token.token_hash%TYPE;
    v_revoked_at access_token.revoked_at%TYPE;
    v_session auth_session.session_id%TYPE;
    v_ended_at auth_session.ended_at%TYPE;
    v_now TIMESTAMP WITH TIME ZONE;
BEGIN
    SELECT token_hash, revoked_at INTO v_hash, v_revoked_at FROM access_token
    WHERE token_hash = :token_hash FOR UPDATE WAIT 1;
    SELECT session_id, ended_at INTO v_session, v_ended_at FROM auth_session
    WHERE session_id = :session_id FOR UPDATE WAIT 1;
    v_now := :now;
    IF v_revoked_at IS NULL AND v_ended_at IS NULL THEN
        UPDATE access_token
        SET expires_at = GREATEST(expires_at, v_now + INTERVAL '30' MINUTE)
        WHERE token_hash = v_hash;
        UPDATE auth_session
        SET last_seen = GREATEST(last_seen, v_now)
        WHERE session_id = v_session;
    END IF;
END;
