-- auth.select_device: whether the customer has used this device before.
-- Access path: DEVICE primary key.
SELECT COUNT(*) FROM device WHERE customer_id = :customer_id AND device_id = :device_id
