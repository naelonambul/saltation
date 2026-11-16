INSERT INTO driver_parity_t (key_value, amount, stamped) VALUES (:key_value, :amount, :stamped) RETURNING id INTO :id
