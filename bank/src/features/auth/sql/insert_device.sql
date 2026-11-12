-- auth.insert_device: first sighting of a device for a customer.
-- Access path: DEVICE primary key.
INSERT INTO device (customer_id, device_id, first_seen, last_seen) VALUES (:customer_id, :device_id, :now, :now2)
