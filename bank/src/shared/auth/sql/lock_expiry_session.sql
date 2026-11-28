SELECT session_id FROM auth_session WHERE session_id = :session_id FOR UPDATE WAIT 1
