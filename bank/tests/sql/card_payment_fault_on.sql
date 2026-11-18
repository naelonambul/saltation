-- test.card_payment_fault_on: simulate a card event insert failure in the integration schema.
ALTER TABLE outbox ADD CONSTRAINT card_payment_test_fault_ck
CHECK (INSTR(event_json, '__card_outbox_fault__') = 0)
