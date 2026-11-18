-- test.card_payment_fault_off: restore normal event insertion after the failure witness.
ALTER TABLE outbox DROP CONSTRAINT card_payment_test_fault_ck
