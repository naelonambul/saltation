-- auth.touch_device: records a device's latest use.
-- Access path: DEVICE primary key.
UPDATE device SET last_seen = :now WHERE customer_id = :customer_id AND device_id = :device_id
