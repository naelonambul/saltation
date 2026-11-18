-- test.auth_deferred_prepare: controlled activity before an authenticated request.
-- Access path: ACCESS_TOKEN and AUTH_SESSION primary keys.
DECLARE
    v_session auth_session.session_id%TYPE;
    v_now TIMESTAMP WITH TIME ZONE;
BEGIN
    SELECT session_id INTO v_session FROM access_token WHERE token_hash = :hash;
    v_now := :now;
    UPDATE access_token SET expires_at = v_now + INTERVAL '20' MINUTE,
        scope = :scope WHERE token_hash = :hash;
    UPDATE auth_session SET last_seen = v_now - INTERVAL '10' MINUTE
        WHERE session_id = v_session;
END;
