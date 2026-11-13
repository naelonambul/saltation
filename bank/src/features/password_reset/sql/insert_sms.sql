-- password_reset.insert_sms: queues the simulated reset SMS.
-- Access path: SMS_OUTBOX identity primary key.
INSERT INTO sms_outbox (phone, message, occurred_at) VALUES (:phone, :message, :now)
