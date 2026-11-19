-- test.admin_fault_off: remove the test completion constraint.
ALTER TABLE outbox DROP CONSTRAINT admin_test_audit_ck
