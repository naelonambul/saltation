-- Isolate a single request without rejecting any other event. The ULID bind
-- is validated before substitution because Oracle DDL cannot use binds.
BEGIN
  IF NOT REGEXP_LIKE(:request_id, '^[0-9A-HJKMNP-TV-Z]{26}$') THEN
    RAISE_APPLICATION_ERROR(-20001, 'invalid request id');
  END IF;
  EXECUTE IMMEDIATE 'ALTER TABLE outbox ADD CONSTRAINT outcome_refuse CHECK (JSON_VALUE(event_json, ''$.event_type'') <> ''request.completed'' OR JSON_VALUE(event_json, ''$.request_id'') IS NULL OR JSON_VALUE(event_json, ''$.request_id'') <> ''' || :request_id || ''')';
END;
