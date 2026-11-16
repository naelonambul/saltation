-- Same envelope and payload as rollover::job_completed; unique ULIDs supplied by the test.
INSERT INTO outbox (event_id, event_json)
SELECT :event_id,
       JSON_OBJECT(
         'schema' VALUE 'saltation.log/1', 'event_id' VALUE :json_event_id,
         'event_type' VALUE 'rollover.job_completed',
         'occurred_at' VALUE :occurred_at, 'recorded_at' VALUE :recorded_at,
         'request_id' VALUE NULL, 'session_id' VALUE NULL, 'device_id' VALUE NULL,
         'customer_no' VALUE NULL, 'account_no' VALUE NULL,
         'channel' VALUE 'SYSTEM', 'listener' VALUE NULL, 'ip' VALUE NULL,
         'region' VALUE NULL, 'outcome' VALUE 'completed', 'rsp_code' VALUE NULL,
         'bank_rsp_code' VALUE NULL, 'human' VALUE 'false' FORMAT JSON,
         'payload' VALUE JSON_OBJECT('business_date' VALUE :business_date, 'job' VALUE 'driver_bench'))
FROM dual
