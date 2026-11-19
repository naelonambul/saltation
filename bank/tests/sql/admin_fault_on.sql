-- test.admin_fault_on: fail the matching completion insert but allow the 503 retry audit.
ALTER TABLE outbox ADD CONSTRAINT admin_test_audit_ck CHECK (
  NOT (
    INSTR(event_json, '"event_type":"request.completed"') > 0
    AND (
      INSTR(event_json, '"request_id":"000000000000000000000000A1"') > 0
      OR INSTR(event_json, '"request_id":"000000000000000000000000A2"') > 0
      OR INSTR(event_json, '"request_id":"000000000000000000000000A3"') > 0
    )
    AND (
      INSTR(event_json, '"http_status":200') > 0
      OR INSTR(event_json, '"http_status":401') > 0
    )
  )
) ENABLE NOVALIDATE
