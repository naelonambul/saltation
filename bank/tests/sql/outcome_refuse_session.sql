-- test.outcome_refuse_session: refuse expiry events for one session.
-- Access path: OUTBOX check constraint (test helper).
BEGIN
  IF NOT REGEXP_LIKE(:session_id, '^[0-9A-HJKMNP-TV-Z]{26}$') THEN
    RAISE_APPLICATION_ERROR(-20001, 'invalid session id');
  END IF;
  EXECUTE IMMEDIATE 'ALTER TABLE outbox ADD CONSTRAINT outcome_refuse CHECK (JSON_VALUE(event_json, ''$.payload.session_id'') IS NULL OR JSON_VALUE(event_json, ''$.payload.session_id'') <> ''' || :session_id || ''') ENABLE NOVALIDATE';
END;
