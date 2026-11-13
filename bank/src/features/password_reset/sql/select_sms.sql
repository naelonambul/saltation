-- password_reset.select_sms: reads the newest SMS for a phone.
-- Access path: SMS_OUTBOX_PHONE_IX by phone and highest sms_id.
SELECT message, occurred_at FROM sms_outbox WHERE sms_id = (SELECT MAX(sms_id) FROM sms_outbox WHERE phone = :phone)
