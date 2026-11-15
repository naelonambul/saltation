-- test.atm_credential: inspect PIN credential state without emitting it.
-- Access path: ATM_CREDENTIAL primary key.
SELECT pin_hash, failed_count, TO_CHAR(locked_until, 'YYYYMMDDHH24MISS')
FROM atm_credential WHERE account_no = :account_no
