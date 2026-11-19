-- test.admin_clock_state: preserve the actual clock storage across the controlled expiry witness.
SELECT clock_mode, business_time, rolled_through FROM sim_clock WHERE id = 'X'
