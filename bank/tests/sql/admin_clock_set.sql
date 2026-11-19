-- test.admin_clock_set: change business time only in this serial integration fixture.
UPDATE sim_clock SET clock_mode = :clock_mode, business_time = :business_time,
  rolled_through = :rolled_through WHERE id = 'X'
